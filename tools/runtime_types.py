"""Validate JSON received by the bounded runtime helpers."""

from dataclasses import dataclass
import json
from typing import TypeAlias, TypeIs, cast

JsonValue: TypeAlias = str | int | float | bool | None | list["JsonValue"] | dict[str, "JsonValue"]
JsonObject: TypeAlias = dict[str, JsonValue]


def _object_list(value: object) -> TypeIs[list[object]]:
    return isinstance(value, list)


def _object_dict(value: object) -> TypeIs[dict[object, object]]:
    return isinstance(value, dict)


def _json_value(value: object) -> JsonValue:
    if value is None or isinstance(value, (str, int, float, bool)):
        return value
    if _object_list(value):
        return [_json_value(item) for item in value]
    if _object_dict(value):
        result: JsonObject = {}
        for key, item in value.items():
            if not isinstance(key, str):
                raise ValueError("JSON object keys must be strings")
            result[key] = _json_value(item)
        return result
    raise ValueError("unsupported JSON value")


def parse_json(text: str | bytes) -> JsonValue:
    # json.loads is untyped; every Python value is an object before validation.
    return _json_value(cast(object, json.loads(text)))


def json_object(value: JsonValue) -> JsonObject:
    if not isinstance(value, dict):
        raise ValueError("expected a JSON object")
    return value


def string_field(value: JsonObject, key: str) -> str:
    field = value[key]
    if not isinstance(field, str):
        raise ValueError(f"{key} must be a string")
    return field


def integer_field(value: JsonObject, key: str) -> int:
    field = value[key]
    if not isinstance(field, int) or isinstance(field, bool):
        raise ValueError(f"{key} must be an integer")
    return field


@dataclass(frozen=True)
class ContainerIdentity:
    container_id: str
    labels: dict[str, str]


@dataclass(frozen=True)
class ContainerState:
    running: bool
    exit_code: int


def _container_object(text: str) -> JsonObject:
    containers = parse_json(text)
    if not isinstance(containers, list) or not containers:
        raise ValueError("docker inspect must return a nonempty array")
    return json_object(containers[0])


def parse_container_identity(text: str) -> ContainerIdentity:
    info = _container_object(text)
    config = json_object(info["Config"])
    raw_labels = json_object(config["Labels"])
    labels = {key: string_field(raw_labels, key) for key in raw_labels}
    return ContainerIdentity(string_field(info, "Id"), labels)


def parse_container_state(text: str) -> ContainerState:
    state = json_object(_container_object(text)["State"])
    running = state["Running"]
    if not isinstance(running, bool):
        raise ValueError("container Running must be a boolean")
    return ContainerState(running, integer_field(state, "ExitCode"))
