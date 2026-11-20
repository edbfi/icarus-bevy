# SPDX-License-Identifier: AGPL-3.0-or-later
"""A minimal typed client for the parts of Hindsight 1.2.3's REST API the memory tool uses.

- Every tag filter is strict: a ``Tags`` leaf matches ``all_strict`` or ``any_strict`` and nothing
  here can express ``any``, ``all``, ``exact`` or fuzzy resolution, so untagged documents never
  leak into a filtered read.
- Every failure is a ``HindsightError``: ``NotFound`` (404), ``Unavailable`` (no connection or a
  timeout) or ``Rejected`` (any other non-2xx answer, or an answer of the wrong shape). Messages name
  the method, the path and the status, never the key.
- The key travels only as the ``Authorization`` header of loopback requests; environment proxies are
  ignored. The ``Connection`` comes from ``service.connection()`` and never shows the key in ``repr``.
"""

from __future__ import annotations

import json
import socket
import time
import urllib.error
import urllib.parse
import urllib.request
from http.client import HTTPResponse
from collections.abc import Mapping, Sequence
from dataclasses import dataclass, field
from typing import cast, override

from tools.memory.model import JSON, is_array, is_object
from tools.memory.service import Connection

API = "/v1/default/banks"
TIMEOUT = 120.0


class HindsightError(RuntimeError):
    """A request failed; the message names method, path and status, never the key."""


class NotFound(HindsightError):
    pass


class Unavailable(HindsightError):
    pass


class Rejected(HindsightError):
    pass


@dataclass(frozen=True)
class Tags:
    """A strict tag leaf: every tag (``every=True``, ``all_strict``) or any of them (``any_strict``)."""

    tags: tuple[str, ...]
    every: bool = True


@dataclass(frozen=True)
class Not:
    group: Group


@dataclass(frozen=True)
class AnyOf:
    groups: tuple[Group, ...]


@dataclass(frozen=True)
class AllOf:
    groups: tuple[Group, ...]


type Group = Tags | Not | AnyOf | AllOf


def group_json(group: Group) -> JSON:
    match group:
        case Tags(tags, every):
            if not tags:
                raise ValueError("a tag leaf names at least one tag")
            return {"tags": list(tags), "match": "all_strict" if every else "any_strict"}
        case Not(inner):
            return {"not": group_json(inner)}
        case AnyOf(groups):
            return {"or": [group_json(item) for item in groups]}
        case AllOf(groups):
            return {"and": [group_json(item) for item in groups]}


@dataclass(frozen=True)
class Item:
    """One retain item: one document. ``timestamp`` is ISO-8601 or ``unset``."""

    document_id: str
    content: str
    tags: tuple[str, ...]
    metadata: Mapping[str, str]
    timestamp: str
    entities: tuple[str, ...]
    entity_types: Mapping[str, str] = field(default_factory=dict[str, str])

    def to_json(self) -> JSON:
        return {
            "content": self.content, "document_id": self.document_id, "tags": list(self.tags),
            "metadata": dict[str, JSON](self.metadata), "timestamp": self.timestamp,
            "entities": [{"text": name, "type": self.entity_types.get(name, "identifier")} for name in self.entities],
            "resolve_entities": False,
        }


@dataclass(frozen=True)
class Document:
    """A bank document. ``text`` is ``None`` in list rows, which carry no text."""

    id: str
    text: str | None
    content_hash: str | None
    tags: tuple[str, ...]
    metadata: dict[str, str]
    entities: tuple[str, ...]
    event_date: str | None


@dataclass(frozen=True)
class Unit:
    """A memory unit (a ``world`` fact of a chunks-mode document)."""

    id: str
    document_id: str | None
    text: str
    kind: str | None
    tags: tuple[str, ...]


@dataclass(frozen=True)
class Hit:
    """One recall result."""

    document_id: str | None
    text: str
    tags: tuple[str, ...]
    metadata: dict[str, str]
    score: float | None


def _object(value: object, what: str) -> dict[str, object]:
    if not is_object(value):
        raise Rejected(f"{what}: expected a JSON object")
    return {str(key): item for key, item in value.items()}


def _list(value: object, what: str) -> list[object]:
    if not is_array(value):
        raise Rejected(f"{what}: expected a JSON array")
    return value


def _text(value: object, what: str) -> str:
    if not isinstance(value, str):
        raise Rejected(f"{what}: expected a string")
    return value


def _optional_text(value: object, what: str) -> str | None:
    return None if value is None else _text(value, what)


def _strings(value: object, what: str) -> tuple[str, ...]:
    return tuple(_text(item, what) for item in _list(value, what)) if value is not None else ()


def _metadata(value: object, what: str) -> dict[str, str]:
    if value is None:
        return {}
    return {key: _text(item, what) for key, item in _object(value, what).items()}


def _document(raw: dict[str, object], what: str) -> Document:
    params = _object(raw["retain_params"], what) if raw.get("retain_params") is not None else {}
    entities = tuple(_text(_object(item, what).get("text"), what) for item in _list(params.get("entities") or [], what))
    return Document(
        id=_text(raw.get("id"), what),
        text=_optional_text(raw.get("original_text"), what),
        content_hash=_optional_text(raw.get("content_hash"), what),
        tags=tuple(sorted(_strings(raw.get("tags"), what))),
        metadata=_metadata(raw.get("document_metadata"), what),
        entities=tuple(sorted(entities)),
        event_date=_optional_text(params.get("event_date"), what),
    )


class Client:
    """REST calls against one bank. ``page`` bounds list pages; ``poll`` the operation polling interval."""

    def __init__(self, connection: Connection, *, timeout: float = TIMEOUT, page: int = 500, poll: float = 0.5) -> None:
        self._connection: Connection = connection
        self._timeout: float = timeout
        self._page: int = page
        self._poll: float = poll
        self._opener: urllib.request.OpenerDirector = urllib.request.build_opener(urllib.request.ProxyHandler({}))

    @override
    def __repr__(self) -> str:
        return f"Client(url={self._connection.url!r}, bank={self._connection.bank!r})"

    @property
    def bank(self) -> str:
        return self._connection.bank

    def _bank_path(self, suffix: str) -> str:
        return f"{API}/{urllib.parse.quote(self._connection.bank, safe='')}{suffix}"

    def _call(self, method: str, path: str, body: JSON = None, query: Sequence[tuple[str, str]] = ()) -> object:
        target = path + (f"?{urllib.parse.urlencode(list(query))}" if query else "")
        request = urllib.request.Request(
            self._connection.url + target,
            data=None if body is None else json.dumps(body, ensure_ascii=False).encode(),
            method=method,
            headers={"Authorization": f"Bearer {self._connection.key}", "Content-Type": "application/json", "Accept": "application/json"},
        )
        label = f"{method} {path}"
        try:
            # OpenerDirector.open is typed Any; for an http URL it returns an HTTPResponse.
            with cast(HTTPResponse, self._opener.open(request, timeout=self._timeout)) as response:
                data = response.read()
        except urllib.error.HTTPError as error:
            detail = error.read()[:300].decode("utf-8", "replace")
            error.close()
            kind = NotFound if error.code == 404 else Rejected
            raise kind(f"{label}: HTTP {error.code}: {detail}") from None
        except (urllib.error.URLError, socket.timeout, ConnectionError) as error:
            reason = error.reason if isinstance(error, urllib.error.URLError) else error
            raise Unavailable(f"{label}: service unavailable ({reason})") from None
        try:
            return cast(object, json.loads(data)) if data else None
        except json.JSONDecodeError:
            raise Rejected(f"{label}: answer is not JSON") from None

    def _pages(self, path: str, query: Sequence[tuple[str, str]], key: str) -> list[dict[str, object]]:
        rows: list[dict[str, object]] = []
        while True:
            answer = _object(self._call("GET", path, query=[*query, ("limit", str(self._page)), ("offset", str(len(rows)))]), path)
            batch = [_object(item, path) for item in _list(answer.get(key), path)]
            rows.extend(batch)
            if len(batch) < self._page:
                return rows

    # Documents.

    def retain(self, items: Sequence[Item]) -> None:
        """Retain ``items`` synchronously, each replacing its document."""
        if items:
            _ = self._call("POST", self._bank_path("/memories"), {"items": [item.to_json() for item in items], "async": False})

    def retain_async(self, items: Sequence[Item]) -> str:
        """Submit ``items`` as one async operation; returns its operation ID."""
        answer = _object(self._call("POST", self._bank_path("/memories"), {"items": [item.to_json() for item in items], "async": True}), "retain")
        return _text(answer.get("operation_id"), "retain operation_id")

    def wait(self, operations: Sequence[str], deadline: float = 3600.0) -> None:
        """Poll until every operation completes; ``Rejected`` on a failed or cancelled one, ``Unavailable`` past ``deadline`` seconds."""
        pending = list(operations)
        stop = time.monotonic() + deadline
        while pending:
            operation = pending[0]
            answer = _object(self._call("GET", self._bank_path(f"/operations/{urllib.parse.quote(operation, safe='')}")), "operation")
            status = _text(answer.get("status"), "operation status")
            if status == "completed":
                _ = pending.pop(0)
                continue
            if status not in ("pending", "processing"):
                raise Rejected(f"operation {operation}: {status}: {answer.get('error_message')}")
            if time.monotonic() > stop:
                raise Unavailable(f"operation {operation}: still {status} after {deadline:.0f} s")
            time.sleep(self._poll)

    def document(self, document_id: str) -> Document | None:
        path = self._bank_path(f"/documents/{urllib.parse.quote(document_id, safe='@')}")
        try:
            return _document(_object(self._call("GET", path), path), path)
        except NotFound:
            return None

    def delete(self, document_id: str) -> None:
        path = self._bank_path(f"/documents/{urllib.parse.quote(document_id, safe='@')}")
        try:
            _ = self._call("DELETE", path)
        except NotFound:
            pass

    def retag(self, document_id: str, tags: Sequence[str]) -> None:
        """Replace a document's tags (propagated to its units); never empty."""
        if not tags:
            raise ValueError("a document keeps at least one tag")
        _ = self._call("PATCH", self._bank_path(f"/documents/{urllib.parse.quote(document_id, safe='@')}"), {"tags": list(tags)})

    def documents(self, tags: Tags | None = None) -> list[Document]:
        """Every document (no filter), or those matching one strict tag leaf; list rows carry no text."""
        query: list[tuple[str, str]] = []
        if tags is not None:
            query = [*(("tags", tag) for tag in tags.tags), ("tags_match", "all_strict" if tags.every else "any_strict")]
        path = self._bank_path("/documents")
        return [_document(row, path) for row in self._pages(path, query, "items")]

    def units(self, *, text: str | None = None, document_id: str | None = None) -> list[Unit]:
        """Memory units, optionally by substring of their text or by document; never filtered by tag."""
        query = [(name, value) for name, value in (("q", text), ("document_id", document_id)) if value is not None]
        path = self._bank_path("/memories/list")
        return [
            Unit(_text(row.get("id"), path), _optional_text(row.get("document_id"), path), _text(row.get("text"), path),
                 _optional_text(row.get("fact_type"), path), tuple(sorted(_strings(row.get("tags"), path))))
            for row in self._pages(path, query, "items")
        ]

    def recall(self, query: str, groups: Sequence[Group], *, query_timestamp: str, max_tokens: int = 4096) -> list[Hit]:
        """World facts ranked for ``query`` under strict tag groups (AND-ed), with a pinned ``query_timestamp``."""
        if not groups:
            raise ValueError("recall always filters by tag groups")
        body: JSON = {
            "query": query, "types": ["world"], "budget": "mid", "max_tokens": max_tokens, "query_timestamp": query_timestamp,
            "tag_groups": [group_json(group) for group in groups], "include": {"entities": None},
        }
        path = self._bank_path("/memories/recall")
        answer = _object(self._call("POST", path, body), path)
        hits: list[Hit] = []
        for item in _list(answer.get("results"), path):
            row = _object(item, path)
            scores = _object(row.get("scores") or {}, path)
            final = scores.get("final")
            hits.append(Hit(
                _optional_text(row.get("document_id"), path), _text(row.get("text"), path), tuple(sorted(_strings(row.get("tags"), path))),
                _metadata(row.get("metadata"), path), float(final) if isinstance(final, (int, float)) and not isinstance(final, bool) else None,
            ))
        return hits

    # Instance assertions.

    def config(self) -> tuple[dict[str, object], dict[str, object]]:
        """The bank's effective config and its bank-level overrides."""
        path = self._bank_path("/config")
        answer = _object(self._call("GET", path), path)
        return _object(answer.get("config"), path), _object(answer.get("overrides") or {}, path)

    def banks(self) -> tuple[str, ...]:
        rows = self._pages(API, (), "banks")
        return tuple(sorted(_text(row.get("bank_id"), API) for row in rows))

    def count(self, kind: str) -> int:
        """How many mental models, directives, webhooks or page-tree roots the bank holds."""
        suffix, key = {"mental-models": ("/mental-models", "items"), "directives": ("/directives", "items"),
                       "webhooks": ("/webhooks", "items"), "pages": ("/pages/tree", "roots")}[kind]
        path = self._bank_path(suffix)
        if kind == "pages":
            return len(_list(_object(self._call("GET", path), path).get(key), path))
        return len(self._pages(path, (), key))
