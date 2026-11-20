# SPDX-License-Identifier: AGPL-3.0-or-later
"""The project's memory tool, ``python3 -m tools.memory`` (rules: ``.agents/rules/memory.md``).

Offline core, stdlib only:

- ``model``: typed records and strict JSON parsing.
- ``canonical``: canonical JSON, sha256, text rendering, tags and entities.
- ``gitio``: the few Git reads every other module shares.
- ``journal``: segment files, their atomic writer and strict readers.
- ``state``: deterministic replay of segments, derived links and partitions.
- ``check``: ``memory check --ci``.
- ``paths``: ``memory protected-paths``.
- ``cli``: argument parsing and output.
"""
