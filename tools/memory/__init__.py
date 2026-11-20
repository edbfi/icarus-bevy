# SPDX-License-Identifier: AGPL-3.0-or-later
"""The project's memory tool, ``python3 -m tools.memory`` (rules: ``.agents/rules/memory.md``).

Stdlib only. The offline core:

- ``model``: typed records and strict JSON parsing.
- ``canonical``: canonical JSON, sha256, text rendering, tags and entities.
- ``gitio``: the few Git reads every other module shares.
- ``journal``: segment files, their atomic writer and strict readers.
- ``state``: deterministic replay of segments, derived links and partitions.
- ``check``: ``memory check --ci``.
- ``paths``: ``memory protected-paths``.

On top of it:

- ``service``: the systemd unit, the one connection file, state directories (``home``) and the host lock.
- ``hindsight``: the typed REST client of the bank.
- ``bank``: the bank as a function of the journal (gather, desired documents, reconcile, ID allocation).
- ``online``: the commands that touch the bank.
- ``durability``: ``backup``, ``restore`` and the drill, with their journal watermark and replay.
- ``mcp``: the stdio MCP server over the same command functions.
- ``cli``: argument parsing and the single handler dispatch.
"""
