# SPDX-License-Identifier: AGPL-3.0-or-later
"""Bounded HTTP transport for the pinned Hindsight document and read APIs."""

from __future__ import annotations

import re
from dataclasses import dataclass
from http.client import HTTPConnection, HTTPException
from os import environ
from time import monotonic, sleep
from typing import final
from urllib.parse import urlsplit
from uuid import uuid4

from tools.docs_lint import parse_json_object
from tools.evidence_catalog_model import as_list, as_object, as_text
from tools.evidence_hindsight_projection import Projection, Row, compact

MAX_RESPONSE = 2_000_000
CONTROL_TIMEOUT = 12.0
RECALL_TIMEOUT = 30.0
# The managed service allows up to 80 seconds for an individual LLM call.
REFLECT_TIMEOUT = 95.0
OPERATION_ID = re.compile(r"[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}")


class HindsightError(ValueError):
    """The local Hindsight endpoint failed or returned an invalid result."""


class HindsightTimeout(HindsightError):
    def __init__(self, seconds: float) -> None:
        super().__init__(f"Hindsight HTTP request timed out after {seconds:g} seconds")


class HindsightHttpError(HindsightError):
    def __init__(self, status: int) -> None:
        super().__init__(f"Hindsight returned HTTP {status}")
        self.status: int = status


@final
class Hindsight:
    def __init__(self, url: str, bank: str) -> None:
        try:
            endpoint = urlsplit(url)
            host, port = endpoint.hostname, endpoint.port
        except ValueError:
            raise HindsightError("expected a loopback HTTP base URL with an explicit port") from None
        if (
            endpoint.scheme != "http" or host not in ("127.0.0.1", "::1") or not port
            or endpoint.username is not None or endpoint.password is not None
            or endpoint.path not in ("", "/") or endpoint.query or endpoint.fragment
        ):
            raise HindsightError("expected a loopback HTTP base URL with an explicit port")
        key = environ.get("HINDSIGHT_API_KEY")
        if not key or not re.fullmatch(r"[\x21-\x7e]+", key):
            raise HindsightError("HINDSIGHT_API_KEY must be set in the environment")
        self._key = key
        self._host = host
        self._port = port
        self._bank = bank

    def require_authenticated_service(self) -> None:
        try:
            _ = self._request("/v1/default/banks", None, "application/json", authenticated=False)
        except HindsightHttpError as error:
            if error.status == 401:
                return
            raise
        raise HindsightError("loopback endpoint allows unauthenticated Hindsight REST access")

    def request(
        self, route: str, data: bytes | None = None, content_type: str = "application/json", *,
        timeout: float = CONTROL_TIMEOUT,
    ) -> dict[str, object]:
        return self._request(f"/v1/default/banks/{self._bank}{route}", data, content_type, timeout=timeout)

    def _request(
        self, path: str, data: bytes | None, content_type: str, *,
        authenticated: bool = True, timeout: float = CONTROL_TIMEOUT,
    ) -> dict[str, object]:
        headers = {"Content-Type": content_type}
        if authenticated:
            headers["Authorization"] = f"Bearer {self._key}"
        connection = HTTPConnection(self._host, self._port, timeout=timeout)
        try:
            connection.request("POST" if data is not None else "GET", path, data, headers)
            response = connection.getresponse()
            if not 200 <= response.status < 300:
                raise HindsightHttpError(response.status)
            body = response.read(MAX_RESPONSE + 1)
        except TimeoutError:
            raise HindsightTimeout(timeout) from None
        except (OSError, HTTPException):
            raise HindsightError("Hindsight connection failed") from None
        finally:
            connection.close()
        if len(body) > MAX_RESPONSE:
            raise HindsightError("Hindsight response exceeds its size limit")
        try:
            return parse_json_object(body)
        except ValueError:
            raise HindsightError("Hindsight response is not a JSON object") from None

    def in_flight_import(self) -> str | None:
        operations: set[str] = set()
        for status in ("pending", "processing"):
            try:
                reply = self.request(f"/operations?type=import_documents&status={status}&limit=2")
            except HindsightHttpError as error:
                if error.status == 404 and not operations:
                    return None  # A fresh bank is created by the import endpoint.
                raise
            listed = object_list(reply.get("operations"))
            if reply.get("bank_id") != self._bank or reply.get("total") != len(listed):
                raise HindsightError("Hindsight did not return a complete import operation listing")
            for entry in listed:
                if entry.get("task_type") != "import_documents" or entry.get("status") != status:
                    raise HindsightError("Hindsight import operation listing is inconsistent")
                operations.add(operation_id(entry.get("id")))
        if len(operations) > 1:
            raise HindsightError("multiple Hindsight imports are already in progress for this bank")
        return next(iter(operations), None)

    def submit_import(self, archive: bytes) -> str:
        if self._key.encode() in archive:
            raise HindsightError("projection contains the configured Hindsight credential")
        boundary = f"catalog-{uuid4().hex}"
        header = (
            f"--{boundary}\r\n"
            'Content-Disposition: form-data; name="file"; filename="projection.zip"\r\n'
            "Content-Type: application/zip\r\n\r\n"
        ).encode()
        data = header + archive + f"\r\n--{boundary}--\r\n".encode()
        reply = self.request(
            "/document-transfer?on_conflict=skip", data, f"multipart/form-data; boundary={boundary}",
        )
        return operation_id(reply.get("operation_id"))

    def transfer(self, archive: bytes, count: int) -> dict[str, object]:
        operation = self.in_flight_import() or self.submit_import(archive)
        deadline = monotonic() + 100
        while monotonic() < deadline:
            response = self.request(f"/operations/{operation}")
            status = response.get("status")
            if response.get("operation_id") != operation or response.get("operation_type") != "import_documents":
                raise HindsightError("Hindsight returned an inconsistent import operation status")
            if status == "completed":
                metadata = as_object(response.get("result_metadata"), "transfer counters")
                keys = ("documents_imported", "documents_skipped", "facts_imported", "observations_imported")
                values = [metadata.get(key) for key in keys]
                imported, skipped, facts, observations = (counter(value, count) for value in values)
                if imported + skipped != count or facts != imported or observations != 0:
                    raise HindsightError("Hindsight transfer counters differ from the reviewed selection")
                return {"operation_id": operation, **dict(zip(keys, values, strict=True))}
            if status not in ("pending", "processing"):
                raise HindsightError(f"Hindsight import operation {operation} did not complete successfully")
            sleep(1)
        raise HindsightError(f"Hindsight import operation {operation} is still unconfirmed")

    def verify_bank(self, projection: Projection) -> None:
        count = len(projection.rows)
        listing = self.request(f"/documents?limit={count + 1}")
        documents = object_list(listing.get("items"))
        if (
            listing.get("total") != count
            or len(documents) != count
            or {as_text(doc.get("id"), "document ID") for doc in documents} != projection.rows.keys()
            or any(doc.get("bank_id") != self._bank or doc.get("memory_unit_count") != 1 for doc in documents)
        ):
            raise HindsightError("Hindsight document inventory differs from the reviewed selection")

        deadline = monotonic() + 180
        for item in projection.rows:
            remaining = deadline - monotonic()
            if remaining <= 0:
                raise HindsightError("exact Hindsight recall exceeded its verification window")
            hits = self.recall(projection, item, (item,), timeout=min(RECALL_TIMEOUT, remaining))
            if len(hits) != 1 or hits[0].row.id != item:
                raise HindsightError(f"Hindsight did not return exactly one matching fact for {item}")

    def recall(
        self, projection: Projection, question: str, items: tuple[str, ...] = (), *, timeout: float = RECALL_TIMEOUT,
    ) -> tuple[Hit, ...]:
        request: dict[str, object] = {
            "query": question, "types": ["world"], "max_tokens": 4096,
            "include": {"chunks": {}}, "rerank": {"prune": False, "dedup": False},
        }
        if items:
            request.update({"tags": [f"id:{item}" for item in items], "tags_match": "any_strict"})
        reply = self.request("/memories/recall", compact(request), timeout=timeout)
        hits = object_list(reply.get("results"))
        if not hits:
            return ()
        chunks = as_object(reply.get("chunks"), "recall chunks")
        matched = tuple(verify_hit(projection, hit, chunks) for hit in hits)
        if len({hit.row.id for hit in matched}) != len(matched) or any(
            items and hit.row.id not in items for hit in matched
        ):
            raise HindsightError("Hindsight returned duplicate or out-of-scope recall candidates")
        return matched

    def draft(self, question: str, hits: tuple[Hit, ...]) -> tuple[str, list[str]]:
        allowed = {hit.fact_id: hit.row for hit in hits}
        reply = self.request("/reflect", compact({
            "query": question, "tags": [f"id:{hit.row.id}" for hit in hits],
            "tags_match": "any_strict", "include": {"facts": {}},
        }), timeout=REFLECT_TIMEOUT)
        answer = as_text(reply.get("text"), "draft")
        sources = object_list(as_object(reply.get("based_on"), "draft citations").get("memories"))
        cited: list[str] = []
        for source in sources:
            row = allowed.get(as_text(source.get("id"), "draft source ID"))
            if row is None or source.get("text") != row.text:
                raise HindsightError("draft cites a fact outside verified recall")
            cited.append(row.id)
        if not cited or not answer.strip() or self._key in answer or len(answer) > 12_000:
            raise HindsightError("draft is missing, too large or lacks verified citations")
        return answer, list(dict.fromkeys(cited))


@dataclass(frozen=True)
class Hit:
    row: Row
    fact_id: str


def operation_id(value: object) -> str:
    if not isinstance(value, str) or not OPERATION_ID.fullmatch(value):
        raise HindsightError("Hindsight did not return a valid import operation ID")
    return value


def counter(value: object, count: int) -> int:
    if type(value) is not int or not 0 <= value <= count:
        raise HindsightError("invalid Hindsight transfer counters")
    return value


def object_list(raw: object) -> list[dict[str, object]]:
    return [as_object(item, "Hindsight result") for item in as_list(raw, "Hindsight results")]


def verify_hit(projection: Projection, hit: dict[str, object], chunks: dict[str, object]) -> Hit:
    metadata = as_object(hit.get("metadata"), "recall metadata")
    row = projection.rows.get(as_text(metadata.get("item_id"), "recall catalog ID"))
    if row is None:
        raise HindsightError("Hindsight returned an unreviewed catalog ID")
    expected = projection.document(row)
    fact = expected["facts"][0]
    fact_id = as_text(hit.get("id"), "recall fact ID")
    chunk_id = as_text(hit.get("chunk_id"), "recall chunk ID")
    chunk = as_object(chunks.get(chunk_id), "recall chunk")
    if (
        not fact_id or not chunk_id or hit.get("document_id") != expected["id"]
        or hit.get("type") != fact["fact_type"] or hit.get("text") != fact["text"]
        or metadata != fact["metadata"] or chunk.get("text") != expected["chunks"][0]["chunk_text"]
    ):
        raise HindsightError("Hindsight recall differs from the reviewed Git record")
    return Hit(row, fact_id)
