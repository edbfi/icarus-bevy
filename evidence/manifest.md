# Evidence manifest

One row per retained artifact. Class `local-private` means the file lives under
ignored `work/`, `reference-data/` or `icarus-client-orig/` on the analyst's machine and is never
deleted by cleanup; `external` means it is held outside this repository or is a digest without
a recorded path. `python3 tools/docs_lint.py --local` verifies local-private digests when stated.
Rows were extracted from fabd2f9:docs/implementation-status.md on 2026-10-13.

| ID | Path | SHA-256 | Date | Producing session | Class |
| --- | --- | --- | --- | --- | --- |
| M-001 | work/native-capture-transition-20261013/ | unstated | 2026-10-13 | Current paused checkpoint — 2026-10-13, 06:28 UTC | local-private |
| M-002 | release library (af4270b native capture fix) | 744fcb62bfbcbbace33fa73cb1134adb466c3f617c0f7c028bb899e8b386a183 | 2026-10-13 | Native capture completeness — 2026-10-13, 04:38 UTC | external |
| M-003 | work/c001-standalone-linear-fixtures-v2-20261013/author-freeze.json | 4c9e8ca21314d595de4064aafa59399518fac1398bb9ac54ce38bc5705f40668 | 2026-10-13 | Current paused checkpoint — 2026-10-13, 06:28 UTC | local-private |
| M-004 | work/c001-standalone-api-validation-v2-20261013/ | unstated | 2026-10-13 | Current paused checkpoint — 2026-10-13, 06:28 UTC | local-private |
| M-005 | work/older-control-owner-next-20261013/PROPOSAL.md | unstated | 2026-10-13 | Current paused checkpoint — 2026-10-13, 06:28 UTC | local-private |
| M-006 | work/older-control-owner-routing-20261013/binary-session.json | unstated | 2026-10-13 | Current paused checkpoint — 2026-10-13, 06:28 UTC | local-private |
| M-007 | work/c001-constructor-listing-diagnostic-20261013/ | unstated | 2026-10-13 | C001 diagnostic observation — 2026-10-13, 05:12 UTC | local-private |
| M-008 | work/c001-constructor-listing-20261013/ | unstated | 2026-10-13 | C001 retained listing rejection — 2026-10-13, 04:53 UTC | local-private |
| M-009 | work/native-capture-transition-20261013/transition.json | unstated | 2026-10-13 | Native capture completeness — 2026-10-13, 04:38 UTC | local-private |
| M-010 | work/native-capture-completeness-20261013/ | unstated | 2026-10-13 | Native capture completeness — 2026-10-13, 04:38 UTC | local-private |
| M-011 | work/c001-constructor-ranges-20261013/run/ranges.json | 9f8191a0c0fa32d06942f9373580a02768b4ebed1750a4d6bc45ecfa9ae72624 | 2026-10-13 | C001 constructor metadata bounds — 2026-10-13, 04:18 UTC; payload | local-private |
| M-012 | work/control-client-constructor-continuation-ranges-20261013/run/ranges.json | d9fbb94daeccd5f1d8bb7510d87005a29bb78baee0ab5516abf2b30979688b9f | 2026-10-13 | Constructor continuation metadata and C001 references — 2026-10-13, 04:03 UTC; payload | local-private |
| M-013 | work/c001-incoming-getter-20261013/result/incoming-references.tsv | 589c493c56039973eff41b1c1399f15df9ab7f761908c3d39b0d6a3541d0e58c | 2026-10-13 | Constructor continuation metadata and C001 references — 2026-10-13, 04:03 UTC; reference TSV | local-private |
| M-014 | work/control-client-constructor-instructions-preparation-v2-20261013/independent-engine-results-review.md | 960e7f1ed944f65f06e33992fccce6976bbb96a1ad5331cdb77c13e7d92e0f53 | 2026-10-13 | Bounded decoder validation — 2026-10-13, 03:32 UTC; independent engine interpretation | local-private |
| M-015 | work/rea-live-connection-20261013/binary-session.json | f513b499fef466e842e41217f29c9c70a21db60f730da91ce976fbf30748b9c3 | 2026-10-13 | Live REA connection — 2026-10-13, 01:46 UTC | local-private |
| M-016 | work/c009-top-leaf-values-20261012/ | unstated | 2026-10-12 | Historical paused checkpoint — 2026-10-12, 21:36 UTC | local-private |
| M-017 | execution-v2/ | unstated | 2026-10-12 | Historical paused checkpoint — 2026-10-12, 21:36 UTC | local-private |
| M-018 | release library (437bfe5 Int-array increment) | 84b50475c357c30c225b96924fb7f9e4c3200c3ae4ca5b0ff48d3d00205e07ca | 2026-10-12 | Historical paused checkpoint — 2026-10-12, 21:36 UTC | external |
| M-019 | work/c009-common-child-selection-20261012/summary.json | 8bcbc1d9b57cad46ab40e820465e46021b74ef44925115a3e28389c1b1fdec7d | 2026-10-12 | Historical paused checkpoint — 2026-10-12, 21:36 UTC; summary | local-private |
| M-020 | PREPARED.md | unstated | 2026-10-12 | Historical paused checkpoint — 2026-10-12, 21:36 UTC | local-private |
| M-021 | failure-analysis.md | unstated | 2026-10-12 | Historical paused checkpoint — 2026-10-12, 21:36 UTC | local-private |
| M-022 | rejected-attempt.json | unstated | 2026-10-12 | Historical paused checkpoint — 2026-10-12, 21:36 UTC | local-private |
| M-023 | work/control-world-secondary-store-20261013/run/instructions.json | c6637e7d8b68187ea5947c30ce89b22489c06d83eab8d4e25794876cddd90073 | 2026-10-13 | World named local table pair — 2026-10-13, 06:12 UTC; payload | local-private |
| M-024 | work/control-world-secondary-store-20261013/notes/independent-postrun-analysis.md | 09206f600bb385720cf9487309d744887fb21750cf68c65787d9fb34d3d70653 | 2026-10-13 | World named local table pair — 2026-10-13, 06:12 UTC; additive interpretation | local-private |
| M-025 | work/control-world-table-bridge-20261013/run/instructions.json | d173899c02d111245fc5a51ac5f84411e86c4a18c471f423103f8f116220b93a | 2026-10-13 | World local-store partial result — 2026-10-13, 05:15 UTC; payload | local-private |
| M-026 | work/control-world-table-bridge-20261013/notes/independent-postrun-analysis.md | c0f1c52c99d8c007c0595f95ebed96b891365341a2e6394a32175da563cc1777 | 2026-10-13 | World local-store partial result — 2026-10-13, 05:15 UTC; additive interpretation | local-private |
| M-027 | work/control-client-constructor-continuation-instructions-20261013/run/instructions.json | 1aeb670aa28494e50fa5a936b3a9728370245913b0c6ffc663e74f26df1e3e69 | 2026-10-13 | Current-client continuation instructions — 2026-10-13, 04:28 UTC; payload | local-private |
| M-028 | work/control-client-constructor-continuation-instructions-20261013/notes/independent-postrun-analysis.md | 223ffe8cab6e51d26bf98835ba2b02a6e796f363ab9e9a3ddb8a1c3962de42af | 2026-10-13 | Current-client continuation instructions — 2026-10-13, 04:28 UTC; additive note | local-private |
| M-029 | work/control-client-constructor-instructions-20261013/run/instructions.json | 7ce6ed4742425bcb1e3d6cd6759586853166078e5c986f1797316ad7b7117815 | 2026-10-13 | Current-client bounded constructor instructions — 2026-10-13, 03:49 UTC; payload | local-private |
| M-030 | work/control-client-constructor-instructions-20261013/notes/independent-postrun-analysis.md | 3eadad29122dbc2d6d6cb49441e0c00a93d94c1e96c2f3a2aefff425da1cc4fd | 2026-10-13 | Current-client bounded constructor instructions — 2026-10-13, 03:49 UTC; additive note | local-private |
| M-031 | work/control-client-constructor-ranges-20261013/run/ranges.json | ff838e96a61252573c95ba09b93f2a4c76d457cefd4a34c6dc727a0752b3b3dd | 2026-10-13 | Current-client constructor metadata bounds — 2026-10-13, 03:04 UTC; payload | local-private |
| M-032 | work/control-client-constructor-ranges-20261013/notes/independent-postrun-analysis.md | 3222b5fbc7de908d4921291bb010618cbe634500422fe93fbfcf329bbf06ae8f | 2026-10-13 | Current-client constructor metadata bounds — 2026-10-13, 03:04 UTC; additive note | local-private |
| M-033 | work/control-client-instruction-routing-20261013/ | unstated | 2026-10-13 | Current-client constructor metadata bounds — 2026-10-13, 03:04 UTC | local-private |
| M-034 | docs/research/archive/2026-10-12-control-caller-limit.md#current-client-public-symbols-and-named-notify-slots | unstated | 2026-10-13 | Current-client public symbols and notify slots — 2026-10-13, 02:31 UTC | external |
| M-035 | docs/research/archive/2026-10-12-control-caller-limit.md#local-post-call-gap-closed | unstated | 2026-10-13 | Post-call local stores — 2026-10-13, 01:35 UTC | external |
| M-036 | docs/research/archive/2026-10-12-control-caller-limit.md#fixed-prefixes-and-base-register-limits | unstated | 2026-10-13 | Control-store prefixes — 2026-10-13, 01:21 UTC | external |
| M-037 | work/protocol-next-increment-20261013/ | unstated | 2026-10-12 | Initial control archive fields — 2026-10-12, 22:24 UTC | local-private |
| M-038 | work/protocol-next-increment-20261013/snapshot-after/target/release/libicarus_bevy.rlib | 7b5430a53f1f252aa639bf037e8952cd11204266d84bda6c0691e240be8d4e38 | 2026-10-12 | Initial control archive fields — 2026-10-12, 22:24 UTC; new release library | local-private |
| M-039 | docs/research/archive/2026-10-12-control-caller-limit.md | unstated | 2026-10-12 | Initial control archive fields — 2026-10-12, 22:24 UTC | external |
| M-040 | work/control-caller-candidates-20261013/candidate-result.json | e7a65b3efc7f0cc5210f526530bc0955b4b47024363b919f1d1289647ba56461 | 2026-10-12 | Initial control archive fields — 2026-10-12, 22:24 UTC; result | local-private |
| M-041 | work/control-dispatch-owner-20261013/owner-result.json | 23a603994a721697446b88cbf66693b07fd95423f1dd68f549c8a3e7c5737df1 | 2026-10-12 | Initial control archive fields — 2026-10-12, 22:24 UTC; result | local-private |
| M-042 | work/control-dispatch-table-references-20261013/references-result.json | eb10c15cb2aec7982956e4a37c2b22214407385b808ff3548b0869d27434f9e7 | 2026-10-12 | Initial control archive fields — 2026-10-12, 22:24 UTC; result | local-private |
| M-043 | work/control-caller-function-ranges-20261013/ranges-result.json | 71f74cc712d962b570f63361dc6cd81d512625e5e72cee69fe4dccf92b717eff | 2026-10-12 | Initial control archive fields — 2026-10-12, 22:24 UTC; result | local-private |
| M-044 | docs/research/archive/2026-10-12-control-caller-limit.md#targeted-instruction-context-and-preserved-failed-attempt | unstated | 2026-10-12 | Initial control archive fields — 2026-10-12, 22:24 UTC | external |
| M-045 | docs/research/archive/2026-10-12-control-caller-limit.md#instruction-only-operand-and-local-store-proof | unstated | 2026-10-12 | Initial control archive fields — 2026-10-12, 22:24 UTC | external |
| M-046 | tools/ | unstated | 2026-10-12 | Initial control archive fields — 2026-10-12, 22:24 UTC; reusable pure PE parser and fixtures | external |
| M-047 | work/c028-coverage-audit-complete-20261013/ | unstated | 2026-10-13 | Selected save coverage audit — 2026-10-13 | local-private |
| M-048 | unstated | 69b002c6648600c2a275d118f4138fc958669068c312cf96a7b13cae0516f185 | 2026-10-13 | Selected save coverage audit — 2026-10-13; corrected neutral map | external |
| M-049 | work/c028-child-leaf-preparation-20261013/summary.json | 5a65b500d1e72610bfd21190be2edbc3a0e41dd3bf6e1cefb42990dbdefd7802 | 2026-10-13 | C028 direct child leaves — 2026-10-13, 03:12 UTC | local-private |
| M-050 | unstated | a71fa4158f4cfe51359b63409655a9c1f9816a0705b00c5d66147ce83c2fde67 | 2026-10-13 | C028 direct child leaves — 2026-10-13, 03:12 UTC; additive note, C028 child leaves | external |
| M-051 | work/c028-all-struct-child-headers-20261013/ | unstated | 2026-10-13 | C028 complete direct Struct-child headers — 2026-10-13, 02:53 UTC | local-private |
| M-052 | work/c028-all-struct-child-headers-20261013/summary-v2.json | df61f3f910216ea3443aa237307d388e97f30f001a1bfa8a15ebc111c2023681 | 2026-10-13 | C028 complete direct Struct-child headers — 2026-10-13, 02:53 UTC; in `work/c028-all-struct-child-headers-20261013/` | local-private |
| M-053 | unstated | 9cb59c06892d3d3484ebc920c854ef47c03b72b0561a34dc37b2d8a326a92495 | 2026-10-13 | C028 complete direct Struct-child headers — 2026-10-13, 02:53 UTC; additive note, C028 Struct-child headers | external |
| M-054 | work/c028-top-string-values-20261013/summary.json | 64c60e41fa3b67527609cf4ff8d2fde3b85a4ae654ff00c51040cfa41aa7db04 | 2026-10-13 | C028 top string structures — 2026-10-13, 02:19 UTC | local-private |
| M-055 | work/c028-top-scalar-selection-20261013/summary.json | 112315a4bb5f9f8c07c90b943899174227f8e7567078989399b90f22bdaed9bf | 2026-10-13 | C028 top scalar range preflight — 2026-10-13, 02:00 UTC | local-private |
| M-056 | work/archive-frame-transition-20261013/ | unstated | 2026-10-13 | Borrowed strict string frames — 2026-10-13 | local-private |
| M-057 | unstated | 1f54114fd2382bb8809071e61929868cd2c836d7002bf717bf92f39adda31549 | 2026-10-13 | Borrowed strict string frames — 2026-10-13; new release library after borrowed-frame change | external |
| M-058 | work/c034-top-scalar-values-20261013/summary.json | edbe6e9c7f0c550a31842ac328f7fa131da3ae0f90bce2b630eb8ba2c7bb3c55 | 2026-10-13 | C034 top scalars — 2026-10-13, 01:24 UTC | local-private |
| M-059 | work/c034-own-scalar-widths-20261013/summary.json | eaaf3fb7382a13fcb3d5e2853dec868ad397ee9885f889d6aa1721b075a21879 | 2026-10-13 | C034 own selected widths — 2026-10-13, 01:06 UTC | local-private |
| M-060 | work/c034-inherited-scalar-values-20261013/summary.json | 54afada3f4608e0d9e11bb1b7c82e774bdc0c34fa653b00f6e5f4cc1c3869187 | 2026-10-13 | C034 inherited scalars — 2026-10-13, 00:50 UTC | local-private |
| M-061 | work/c034-scalar-transitions-20261013/ | unstated | 2026-10-13 | C034 inherited scalars — 2026-10-13, 00:50 UTC | local-private |
| M-062 | work/c009-d01-array-child-leaves-20261013/summary.json | da619895c34bb48cced66efccede2db5ded96d7e96f6ae7166168ceef197c244 | 2026-10-13 | D01 nested array leaves — 2026-10-13, 00:30 UTC | local-private |
| M-063 | work/c009-d01-array-child-headers-20261013/summary.json | 1d4961524431734e7199a0e16c0e1e983448eb594cb1d3b01f789dd729641623 | 2026-10-13 | D01 nested array headers — 2026-10-13, 00:17 UTC | local-private |
| M-064 | work/c009-d01-leaf-values-20261013/summary.json | 6f53c189ea8c33edc7c736b08f5685da14c7bfee93968c7b98ca9878b10e0ee1 | 2026-10-13 | D01 leaves — 2026-10-13, 00:01 UTC | local-private |
| M-065 | work/c028-d01-array-children-20261012/ | unstated | 2026-10-13 | D01 leaves — 2026-10-13, 00:01 UTC | local-private |
| M-066 | work/c009-common-deeper-headers-20261013/summary.json | ede5ffe4a6a1944f13dfd8c51373efa6c5eccd65a41db8454190a689182e3f96 | 2026-10-12 | C009 deeper-array headers — 2026-10-12, 23:48 UTC | local-private |
| M-067 | unstated | unstated | 2026-10-12 | C009 deeper-array headers — 2026-10-12, 23:48 UTC; current library `7b5430…`, prefix only | external |
| M-068 | work/c009-common-scalar-values-20261013/summary.json | 244e4919870e5b0ecf94826c791a67049823fcfa46e38fc774ff0fe5d3e22626 | 2026-10-12 | C009 common-child scalars — 2026-10-12, 23:29 UTC | local-private |
| M-069 | work/c009-common-binary-widths-20261013/summary.json | f7867196f10211aa7d00fb53d38048e7948053eab4ec37507bd6052e694b56a8 | 2026-10-12 | C009 common-child binary widths — 2026-10-12, 23:04 UTC | local-private |
| M-070 | work/c009-common-child-headers-20261012/summary.json | 67939440788b396d31461a1c17bc3b4849a7ec6a26e19b11b5eee3e163e5b95f | 2026-10-12 | C009 common-child headers — 2026-10-12, 22:15 UTC | local-private |
| M-071 | work/array-limit-preparation-20261012/ | unstated | 2026-10-12 | Explicit struct-array limits — 2026-10-12, 22:08 UTC | local-private |
| M-072 | work/array-limit-prechange-20261013/ | unstated | 2026-10-12 | Explicit struct-array limits — 2026-10-12, 22:08 UTC | local-private |
| M-073 | unstated | e9cb457f168ec214c54ad9931074b0f8070f179eecda6f5d79f97a3288ca87d2 | 2026-10-12 | Explicit struct-array limits — 2026-10-12, 22:08 UTC; new release library after struct-array limit change | external |
| M-074 | unstated | unstated | 2026-10-12 | Explicit struct-array limits — 2026-10-12, 22:08 UTC; previous library `84b50475…`, prefix only | external |
| M-075 | work/c009-top-leaf-values-20261012/execution-v3/summary.json | f47801ede31b8c2a78efaad514c708362edd1758560c80f19318f144e7b90c47 | 2026-10-12 | Resumed scalar qualification — 2026-10-12, 22:02 UTC | local-private |
| M-076 | work/dependency-recheck-20261012/ | unstated | 2026-10-12 | Resumed scalar qualification — 2026-10-12, 22:02 UTC | local-private |
| M-077 | work/native-c034-own-20261012/ | unstated | 2026-10-12 | Completed continuation work (under Continuation — 2026-10-12) | local-private |
| M-078 | work/dependency-refresh-20261012/ | unstated | 2026-10-12 | Completed continuation work (under Continuation — 2026-10-12) | local-private |
| M-079 | work/c034-own-array-continuation-20261012/ | unstated | 2026-10-12 | Completed continuation work (under Continuation — 2026-10-12) | local-private |
| M-080 | work/c034-own-array-compare-20261012/ | unstated | 2026-10-12 | Completed continuation work (under Continuation — 2026-10-12) | local-private |
| M-081 | work/c034-value-widths-20261012/ | unstated | 2026-10-12 | Completed continuation work (under Continuation — 2026-10-12) | local-private |
| M-082 | work/client-static-continuation-20261012/ | unstated | 2026-10-12 | Completed continuation work (under Continuation — 2026-10-12) | local-private |
| M-083 | work/client-tag-array-20261012/ | unstated | 2026-10-12 | Completed continuation work (under Continuation — 2026-10-12) | local-private |
| M-084 | work/client-c034-own-array-20261012/ | unstated | 2026-10-12 | Completed continuation work (under Continuation — 2026-10-12) | local-private |
| M-085 | work/client-profile-ops-20261012/ | unstated | 2026-10-12 | Completed continuation work (under Continuation — 2026-10-12) | local-private |
| M-086 | work/client-float-contract-20261012/ | unstated | 2026-10-12 | Completed continuation work (under Continuation — 2026-10-12) | local-private |
| M-087 | work/c034-all-headers-20261012/ | unstated | 2026-10-12 | Completed continuation work (under Continuation — 2026-10-12) | local-private |
| M-088 | work/c034-all-own-headers-20261012/ | unstated | 2026-10-12 | Completed continuation work (under Continuation — 2026-10-12) | local-private |
| M-089 | work/c034-all-own-arrays-20261012/ | unstated | 2026-10-12 | Completed continuation work (under Continuation — 2026-10-12) | local-private |
| M-090 | work/c034-inherited-arrays-20261012/ | unstated | 2026-10-12 | Completed continuation work (under Continuation — 2026-10-12) | local-private |
| M-091 | work/str-value-contract-20261012/ | unstated | 2026-10-12 | Completed continuation work (under Continuation — 2026-10-12) | local-private |
| M-092 | work/c034-inherited-structs-20261012/ | unstated | 2026-10-12 | Completed continuation work (under Continuation — 2026-10-12) | local-private |
| M-093 | work/c034-field-widths-20261012/ | unstated | 2026-10-12 | Completed continuation work (under Continuation — 2026-10-12) | local-private |
| M-094 | work/component-c009-20261012/ | unstated | 2026-10-12 | Completed continuation work (under Continuation — 2026-10-12) | local-private |
| M-095 | work/native-c009-top-20261012/ | unstated | 2026-10-12 | Completed continuation work (under Continuation — 2026-10-12) | local-private |
| M-096 | work/c009-own-children-20261012/ | unstated | 2026-10-12 | Completed continuation work (under Continuation — 2026-10-12) | local-private |
| M-097 | work/native-c009-own-20261012/ | unstated | 2026-10-12 | Completed continuation work (under Continuation — 2026-10-12) | local-private |
| M-098 | work/native-c009-int-array-20261012/execution-v2/ | unstated | 2026-10-12 | Completed continuation work (under Continuation — 2026-10-12) | local-private |
| M-099 | work/c009-own-leaf-values-20261012/ | unstated | 2026-10-12 | Completed continuation work (under Continuation — 2026-10-12) | local-private |
| M-100 | work/c009-parent-children-20261012/ | unstated | 2026-10-12 | Completed continuation work (under Continuation — 2026-10-12) | local-private |
| M-101 | work/native-c009-parent-selection-20261012/ | unstated | 2026-10-12 | Completed continuation work (under Continuation — 2026-10-12) | local-private |
| M-102 | work/native-c009-parent-headers-20261012/ | unstated | 2026-10-12 | Completed continuation work (under Continuation — 2026-10-12) | local-private |
| M-103 | work/c009-parent-leaf-values-20261012/ | unstated | 2026-10-12 | Completed continuation work (under Continuation — 2026-10-12) | local-private |
| M-104 | work/c009-byte-value-contract-20261012/ | unstated | 2026-10-12 | Completed continuation work (under Continuation — 2026-10-12) | local-private |
| M-105 | work/c009-top-enum-contract-20261012/ | unstated | 2026-10-12 | Completed continuation work (under Continuation — 2026-10-12) | local-private |
| M-106 | work/c009-common-child-selection-20261012/ | unstated | 2026-10-12 | Completed continuation work (under Continuation — 2026-10-12) | local-private |
| M-107 | work/c034-name-contract-20261012/ | unstated | 2026-10-12 | Completed continuation work (under Continuation — 2026-10-12) | local-private |
| M-108 | work/c034-name-values-20261012/ | unstated | 2026-10-12 | Completed continuation work (under Continuation — 2026-10-12) | local-private |
| M-109 | commit b55e4ce (checkpoint commits delivered) | unstated | 2026-10-12 | Completed continuation work (under Continuation — 2026-10-12) | external |
| M-110 | commit af8cf48 (Float/Text increment) | unstated | 2026-10-12 | Completed continuation work (under Continuation — 2026-10-12) | external |
| M-111 | commit 93ef9e9 (struct-array increment) | unstated | 2026-10-12 | Completed continuation work (under Continuation — 2026-10-12) | external |
| M-112 | work/native-float-text-20261012/ | unstated | 2026-10-12 | Completed continuation work (under Continuation — 2026-10-12) | local-private |
| M-113 | icarus-client-orig/25793767 | unstated | 2026-10-12 | Completed continuation work (under Continuation — 2026-10-12) | external |
| M-114 | steamapps/appmanifest_1149460.acf | unstated | 2026-10-12 | Completed continuation work (under Continuation — 2026-10-12) | external |
| M-115 | Icarus/Binaries/Win64/ | ab11ec1212127e9047128843e584ab636632353edd6808a0db9caedb35048179 | 2026-10-12 | Completed continuation work (under Continuation — 2026-10-12); client executable | external |
| M-116 | Icarus/Binaries/Win64/ | 944e38c941cdbdf4ec512f4307134fdd307e5e1c61602f06bd2f9600f7f98bef | 2026-10-12 | Completed continuation work (under Continuation — 2026-10-12); client PDB | external |
| M-117 | work/client-identity-20261012/ | unstated | 2026-10-12 | Completed continuation work (under Continuation — 2026-10-12) | local-private |
| M-118 | save-triage-20261012/ | unstated | 2026-10-12 | Completed continuation work (under Continuation — 2026-10-12) | local-private |
| M-119 | c028-array-children-20261012/ | unstated | 2026-10-12 | Completed continuation work (under Continuation — 2026-10-12) | local-private |
| M-120 | work/c028-array-children-20261012/static/header-kind-contracts-v2.json | 17abd2de90aea26129d13cc075c7a6268eef41cf653f14dcca29928433b97e09 | 2026-10-12 | Completed continuation work (under Continuation — 2026-10-12) | local-private |
| M-121 | c028-array-descendants-20261012/ | unstated | 2026-10-12 | Completed continuation work (under Continuation — 2026-10-12) | local-private |
| M-122 | c028-d01-array-children-20261012/ | unstated | 2026-10-12 | Completed continuation work (under Continuation — 2026-10-12) | local-private |
| M-123 | component-c034-20261012/ | unstated | 2026-10-12 | Completed continuation work (under Continuation — 2026-10-12) | local-private |
| M-124 | c034-own-struct-20261012/ | unstated | 2026-10-12 | Completed continuation work (under Continuation — 2026-10-12) | local-private |
| M-125 | work/component-c034-20261012/static/c034-candidate-framing-proof.json | c5541899734b5c967ad4617680219d5778a8981529badcfe25d8ebad4e36212d | 2026-10-12 | Completed continuation work (under Continuation — 2026-10-12) | local-private |
| M-126 | work/component-c034-20261012/static/c034-schema-private.json | e7b04c3629d922b1597f57bf948c1ae6b312764c2b9ebc91b54ac5a41f7f857f | 2026-10-12 | Completed continuation work (under Continuation — 2026-10-12) | local-private |
| M-127 | work/c034-own-struct-20261012/static/own-struct-schema-private.json | 0a9cc629c485777c63f56dd4a20aa21d553d5921436d13af74d11b62c1ab50aa | 2026-10-12 | Completed continuation work (under Continuation — 2026-10-12); own-struct private map; filename unstated | local-private |
| M-128 | work/c028-array-descendants-20261012/static/descendant-contracts-private.json | a39fa352ae09eb2684d05e73f9ec3ba6b811871eac6109bc7cc5c9b201a1a248 | 2026-10-12 | Completed continuation work (under Continuation — 2026-10-12) | local-private |
| M-129 | work/native-struct-array-20261012/probe | 211570ebf49c4bd5e7e150fda3786fd65cf1be52442b1c38f30f502c9607c7fb | 2026-10-12 | Completed continuation work (under Continuation — 2026-10-12) | local-private |
| M-130 | bound release library (native struct-array helper; file path unstated) | 16ee0c084c085ed09d8934a9253c70d6f59ebc9b9763273c8e8603d181f01346 | 2026-10-12 | Historical digest from helper README and checkpoint; no retained library path recorded | external |
| M-131 | edbfi/skills commit 69cc23f | unstated | unstated | Implemented and checked | external |
| M-132 | work/native-deadline-20261012/ | unstated | unstated | Implemented and checked | local-private |
| M-133 | work/icarus-overnight/native-recovery-02/ | unstated | unstated | Implemented and checked | local-private |
| M-134 | work/icarus-overnight/native-soak.jsonl | unstated | unstated | Implemented and checked | local-private |
| M-135 | commands.jsonl | unstated | unstated | Implemented and checked | external |
| M-136 | commit cac76ec (historical soak start) | unstated | unstated | Implemented and checked | external |
| M-137 | docs/research/archive/2026-10-04-native-snapshot-performance.md | unstated | unstated | Implemented and checked | external |
| M-138 | work/native-io-20261012/native-io-faults/ | unstated | unstated | Implemented and checked | local-private |
| M-139 | work/native-codec-review-20261012/ | unstated | unstated | Implemented and checked | local-private |
| M-140 | work/native-tagged-20261012/ | unstated | unstated | Implemented and checked | local-private |
| M-141 | work/native-recorders-20261012/ | unstated | unstated | Implemented and checked | local-private |
| M-142 | work/native-name-header-20261012/ | unstated | unstated | Implemented and checked | local-private |
| M-143 | work/c028-structs-20261012/ | unstated | unstated | Implemented and checked | local-private |
| M-144 | work/native-component-20261012/ | unstated | unstated | Implemented and checked | local-private |
| M-145 | history/offline-runtime-authorization.md | unstated | unstated | Implemented and checked | external |
| M-146 | work/control-version-20261012/ | unstated | unstated | Reference evidence | local-private |
| M-147 | work/save-schema-20261012/ | unstated | unstated | Reference evidence | local-private |
| M-148 | work/component-c028-20261012/ | unstated | unstated | Reference evidence | local-private |
| M-149 | work/c028-binary-children-20261012/ | unstated | unstated | Reference evidence | local-private |
| M-150 | work/c028-arrays-20261012/ | unstated | unstated | Reference evidence | local-private |
| M-151 | reference-data/ | unstated | unstated | Reference evidence | local-private |
| M-152 | docs/specs/network/stateless-handshake.md | unstated | unstated | Reference evidence | external |
| M-153 | docs/specs/network/network-checksum.md | unstated | unstated | Reference evidence | external |
| M-154 | docs/research/archive/2026-10-04-prospect-rejection.md | unstated | unstated | Reference evidence | external |
| M-155 | docs/research/archive/2026-10-04-save-failure-output.md | unstated | unstated | Reference evidence | external |
| M-156 | docs/specs/save-format/prospect-loading.md | unstated | unstated | Reference evidence | external |
| M-157 | docs/research/archive/2026-10-04-reference-inventory.md | unstated | unstated | Reference evidence | external |
| M-158 | docs/research/archive/2026-10-04-static-baseline.md | unstated | unstated | Reference evidence | external |
| M-159 | docs/research/archive/2026-10-04-decompilation-report.md | unstated | unstated | Reference evidence | external |
| M-160 | docs/research/archive/2026-10-04-targeted-static-report.md | unstated | unstated | Reference evidence | external |
| M-161 | docs/specs/network/control-negotiation.md | unstated | unstated | Reference evidence | external |
| M-162 | docs/research/archive/2026-10-04-offline-startup-report.md | unstated | unstated | Reference evidence | external |
| M-163 | docs/specs/save-format/prospect-envelope.md | unstated | unstated | Implemented and checked | external |
| M-164 | docs/specs/save-format/archive-string.md | unstated | unstated | Implemented and checked | external |
| M-165 | bug-ledger.md | unstated | unstated | Implemented and checked | external |
| M-166 | reference-data/2026-10-04-heavy-save/ | unstated | 2026-10-12 | Supplied heavy-world save intake (archive, SHA256SUMS, private inventory.json) | local-private |
| M-167 | work/client-identity-20261012/ | unstated | 2026-10-12 | Client installation identity check | local-private |
| M-168 | icarus-orig/Icarus/Binaries/Win64/IcarusServer-Win64-Shipping.exe | 67cfc915d9d085b758057e2960d4a2018fb121123af1329c25ef7aa58b960104 | 2026-10-04 | Reference inventory; preserved dedicated-server build 25621760, 108,067,840 bytes | local-private |
| M-169 | work/icarus-overnight/ | unstated | 2026-10-04 | Overnight case: decompile-01/05/06, handshake runs run-02-handshake and run-03-stateless-ack, evidence E-004 to E-087 | local-private |
| M-170 | work/icarus-server-baseline/ | unstated | 2026-10-04 | Baseline case: inventory, static reports, control dispatch evidence E-011/E-013/E-021/E-022/E-024, Ghidra export | local-private |
| M-171 | work/save-triage-20261012/recorder-native/ | unstated | 2026-10-12 | Native recorder qualification (save-triage E-018) | local-private |
| M-172 | work/save-triage-20261012/component-c034-headers/ | unstated | 2026-10-12 | C034 bounded top-header comparison (save-triage E-038, strict review 30) | local-private |
| M-173 | work/c001-incoming-getter-20261013/result/manifest.tsv | 715936778e0b83a1404f3d429c91a8fb3fb6da40a38499b44065c7092bddbfc8 | 2026-10-13 | Constructor continuation metadata and C001 references; manifest digest (TSV digest is M-013) | local-private |
| M-174 | work/continuation-readiness-20261013/resumption.json | 2d8a3469c05bdd957115daae34a672dccd8568bf20da24518c7967f3d59ee66a | 2026-10-13 | Execution resumption and upstream/provider recheck | local-private |
| M-175 | work/native-fifo-reproduction-20261013/ | unstated | 2026-10-13 | Authored FIFO startup reproduction and native fix validation | local-private |
| M-176 | work/c001-fixtures-v2-review-continuation-20261013/verdict.md | c2499c9f724633774f4417a2d7a71bf04e9080bac21c801071a6863ea1272190 | 2026-10-13 | Independent corrected C001 fixture source review; adjacent outer verdict/checks retained | local-private |
| M-177 | work/c001-fixtures-v2-execution-continuation-20261013/receipt.json | d478ff614d45ab24f4dd7b258d7fe8b7d04025eaee10f2c6b4ad8ead0077acf3 | 2026-10-13 | Fresh corrected C001 authored engine case; adjacent bindings, replay and cleanup retained | local-private |
| M-178 | work/manifest-reconciliation-20261013/receipt.json | eee378731e43b8fa34f14381f78887e144ffe964ae8d6142d7c6c8ab12bbdc38 | 2026-10-13 | Manifest path reconciliation; 42 exact digest joins and one historical digest classified external | local-private |
| M-179 | work/c001-exact-window-review-20261013/verdict.md | 741fb6091b34e79b842e7828435e3d6f9b006bf0a429885471d614f25a7f5950 | 2026-10-13 | Independent exact C001 target source and scope review | local-private |
| M-180 | work/c001-exact-window-execution-20261013/receipt.json | 58bceb47f4fd0c679265701a511729cb64d3cf5b90e451fbede0c1933a4b1c6b | 2026-10-13 | Exact C001 physical-byte static decode; adjacent joins/bindings/cleanup retained | local-private |
| M-181 | work/c001-local-construction-review-20261013/qualified-receipt.json | ee477b248b2c0665c1e0a37299456495b51a0ca4a2d4cc57823c56a1cbdba4cd | 2026-10-13 | Qualified local C001 typed instruction comparison; no callee/ABI/archive proof | local-private |
| M-182 | work/envelope-admission-reproduction-20261013/validation.json | 66f47b2e4a50955bdb43e896317cdf174f99f4e6317248000dbdad9774afba58 | 2026-10-13 | Authored envelope admission/privacy before-after reproduction; source review and tests retained | local-private |
| M-183 | work/native-fifo-reproduction-20261013/release-smoke/receipt.json | f284d07adb1e151b2f5e3d0fd340403d39bc5bf21f8f292c1271b32a88eec551 | 2026-10-13 | FIFO fix release build, save-restart smoke and release FIFO rejection | local-private |
| M-184 | work/dependency-update-20261013/validation.json | b9c7d6f0f9a5218e57665add09702069320608a557773fbb750bcfc01a488f0a | 2026-10-13 | Stable-compatible dependency refresh; prior source/build preservation and fresh full-suite/release checks | local-private |
| M-185 | work/c001-common-callee-review-20261013/verdict.md | 7b83cedb4144a1c682581d3aeeccd12e34314e4b9c910a45133b552672907b06 | 2026-10-13 | Common callee one-seed metadata selector and wrapper independent source review | local-private |
| M-186 | work/c001-common-callee-metadata-20261013/receipt.json | 826e9415919eb1414ae0d5bae31c78518ac8fd739b60db971361d86a391c37e2 | 2026-10-13 | Bounded common callee exception metadata; adjacent execution/RSS/bindings/input/result joins retained | local-private |
| M-187 | work/c001-common-callee-decoder-review-20261013/verdict.md | 25c81c971ffe2cbe90bc449af7e91ebaf4a816b48fb51ce502456ae4edec752f | 2026-10-13 | Single-window decoder and hardened fixture harness independent source review | local-private |
| M-188 | work/c001-common-callee-fixtures-execution-20261013/receipt.json | d478ff614d45ab24f4dd7b258d7fe8b7d04025eaee10f2c6b4ad8ead0077acf3 | 2026-10-13 | Fresh single-window authored compile/ten-mode engine qualification; adjacent replay/pins/classes/cleanup retained | local-private |
| M-189 | work/c001-common-callee-code-review-20261013/verdict.md | 1100f5d8ded8f0a320d1a084c5dfd865015196960174eb090c5d8e85f050bcb6 | 2026-10-13 | Single106-byte original static case independent source/scope and full-wrapper mock review | local-private |
| M-190 | work/c001-common-callee-code-20261013/receipt.json | 5dd6a19fee512e1f935d294b3e0887a175b8386801aa81819f900f1364295f31 | 2026-10-13 | Exact106-byte common callee decode; adjacent metadata/input/pins/classes/replay/cleanup joins retained | local-private |
| M-191 | work/manifest-digest-reproduction-20261013/validation.json | a05670e84237da872ce968d65943bb672dec574bee32b9ece720c53aa9039426 | 2026-10-13 | Authored malformed manifest digest regression, independent source review and 32 passing foundation/CLI tests | local-private |
| M-192 | work/manifest-digest-reproduction-20261013/release-smoke.json | b9f223eea281d94b12a985cc9b2604e9ed46f855424f49b8e4adebee30428e10 | 2026-10-13 | Twelve authored before/after release manifest admission cases and valid-mismatch recovery; prior binary retained | local-private |
| M-193 | work/c001-common-callee-local-facts-review-20261013/verdict.md | 09e4e8966204dc2f5e3dc3b818f6dcdcd87a8043eb550f2b3f77e7e99d056bcb | 2026-10-13 | Pure typed-call/local-stack checker and bounded replay wrapper independent review | local-private |
| M-194 | work/c001-common-callee-local-facts-review-20261013/sibling-scope-freeze.json | 12436734057fd291a70fe1be90cb84d9a07b3d37c970097196d7cacad139b47b | 2026-10-13 | Additive accepted scope/setup/owned-sibling joins; prior reviews unchanged | local-private |
| M-195 | work/c001-common-callee-local-facts-qualified-20261013/execution.json | a55ed0399c03d63873957d9171f3e6ca1d98a764f7c8e6da3a5e753339c4a76c | 2026-10-13 | Successful actual retained-result qualification, exact facts hash and independent replay/pins | local-private |
| M-196 | work/c001-common-callee-local-facts-20261013/exit-capture-gap.json | 48a7c142e8bdfdb90cdc34f005d8b7386a09eeb87dfdda838596348a76e0f7ea | 2026-10-13 | Earlier candidate preserved unqualified because external exit field was not retained | local-private |
| M-197 | work/c001-nested-callees-review-20261013/qualified-path-verdict.md | c660d5bebc8624575be50c930bf0b88e53444640b9cbfd5259a013aebb0a3192 | 2026-10-13 | Two-seed selector/root-wrapper source review and successful-local-case gate | local-private |
| M-198 | work/c001-nested-callees-metadata-20261013/receipt.json | 49d1b46dd731cb118f163ecc361005507a549f4710a41543adb43ad28b11655b | 2026-10-13 | Both nested direct-call exception intervals; numeric origins, full metadata and independent tuple/mapping joins | local-private |
| M-199 | work/c001-nested-callees-metadata-20261013/execution.json | d4a6e1147db89f1523274bce6082521ecd3090ff30885fb0019d8af0b7f0750a | 2026-10-13 | Actual metadata case exit/resource/result/pin qualification; no code/unwind decoding | local-private |
| M-200 | work/client-save-compatibility-report-20261013/report.json | 745473b621c0e9271ef7d8f2a39ba280b74259215604aa6771a3608de76c81bb | 2026-10-13 | User-reported client/save compatibility and deferred authenticated/reference access | local-private |
| M-201 | work/c001-nested-callees-decoder-review-20261013/final-verdict.md | be322a894d82d0021e4729925a6368b836bc960c1323a74e7720cf8bc42b2459 | 2026-10-13 | Nested two-window decoder independent source review; adjacent corrected final freeze and fixture wrapper addenda retained | local-private |
| M-202 | work/c001-nested-callee-fixtures-execution-20261013/receipt.json | d478ff614d45ab24f4dd7b258d7fe8b7d04025eaee10f2c6b4ad8ead0077acf3 | 2026-10-13 | Fresh API compile and ten actual authored engine modes; adjacent external exit, independent recipes/pins/classes/cleanup | local-private |
| M-203 | work/native-json-diagnostics-fix-review-20261013/verdict.md | e86a2f5eaffc4e92bf4f5c00432724b8979044b72f5db016ea14ecbbcdde1e4b | 2026-10-13 | Shared JSON diagnostic helper and three native boundaries independent source review; regression review adjacent | local-private |
| M-204 | work/native-json-diagnostics-reproduction-20261013/validation.json | e508b19230073fc43658f68da6c6d6c45873587a1b5202c38fc33b70e903b927 | 2026-10-13 | Eight release disclosure reproductions and51targeted after-tests with API/CLI state-preservation coverage | local-private |
| M-205 | work/c001-nested-callees-code-review-20261013/verdict.md | bcca7a6ffffc0f6d07bbad75f4e31205a54d370f772ec6185714e79ae7201df5 | 2026-10-13 | Original nested413-byte source/scope/wrapper independent review; argument-order correction preserved | local-private |
| M-206 | work/c001-nested-callees-code-20261013/receipt.json | c707d3ff7ab90e34f57af891da5e880202bb1195dc1fe573e5bce2b5358e3a2f | 2026-10-13 | Exact196/217-byte original decode; adjacent externalexit/replay/input/source/classes/cleanup joins retained | local-private |
| M-207 | work/native-json-diagnostics-release-review-20261014/verdict.md | b9a032b7fb26b69f336b24425d063f15ccaa599ae83ea5e26e1affbe0369c3fe | 2026-10-13 | Release diagnostic validator independent source review;35checks and16mock scenarios with acquisition identity and both-generation checks | local-private |
| M-208 | work/native-json-diagnostics-release-20261014/receipt.json | 5f4bad7a6c2ca61cd0bbce79fb432c90e8ce840dce38af3f4a577fdb9464936c | 2026-10-13 | Release build and nine malformed-input cases; explicitexit0, stable source/binary/input identities, private markers absent and state unchanged | local-private |
| M-209 | work/c001-nested-callees-local-cfg-review-20261014/verdict.md | e88c0f2b7ee428248e0d627a6cd24ebe66dfa23240fe152443096719debb6ab4 | 2026-10-13 | Pure CFG checker independent review;87authored and98independent typed/schema checks with no target reads | local-private |
| M-210 | work/c001-nested-callees-local-cfg-review-20261014/wrapper-verdict.md | a324185e361126a73a3774888c6f8ff88ec875c39bff6842457e89441542deeb | 2026-10-13 | Retained CFG wrapper independent review;51mock guards, acquired-buffer correction and frozen source/scope identities | local-private |
| M-211 | work/c001-nested-callees-local-cfg-20261014/receipt.json | b51f4c1dd975b9846f042853454c2a52e7a8725dc184d71140d9e173f9c48fea | 2026-10-13 | Explicitexit0 retained413-byte CFG replay with17pins; adjacent launch-path rejection, result and final joins preserved | local-private |
| M-212 | work/native-current-soak-review-20261014/verdict.md | ad4c6a06b0c6f5f36cd4fe08e670cf88ea960c409ea9b2eb19418a8db0e5e619 | 2026-10-13 | Current synthetic workload validator review;33source and24authored VM checks, phase/pin/ack/exit corrections qualified | local-private |
| M-213 | work/native-current-soak-20261014/receipt-v2.json | e2ed9d3457400557065dd7859bd5d971a42fdfecc0f3fffe6cff7aadc3153693 | 2026-10-13 | Current120s native100k workload,60saves and exact both-generation verification; source/binary/affinity/exit/RSS preserved | local-private |
| M-214 | work/native-object-admission-review-20261014/review.md | b0b3a3bb9db1cebe736021671d45702d26c9ec162b70ba043f3577d0d8bcb6e4 | 2026-10-13 | Native positional admission source audit;18authored fixtures and19hash checks; fullworld/header limitation preserved | local-private |
| M-215 | work/native-object-admission-fix-review-20261014/implementation-verdict.md | 36bc25b4c5ca38cc2522955107dbf7c6d6805ada108a5bb531a5fe05c5c022a6 | 2026-10-13 | Shared object visitor and native storage gates source review; type/Serialize/future-schema/recovery constraints | local-private |
| M-216 | work/native-object-admission-fix-review-20261014/tests-verdict.md | 23441cfc1c71a5b9b5d21d734671109792dd744e782947f579881da6513532ef | 2026-10-13 | Four regression tests covering13variants independent source review;44checks, bounded CLI and state/ack joins | local-private |
| M-217 | work/native-object-admission-fix-review-20261014/release-verdict.md | 980251f51ca153d9b2d68731b6bb5e7af13e953a471b14542a770101b4c7241a | 2026-10-13 | Release validator V2 review;28mock scenarios, exact input shapes and source/binary/recovery/checksum joins | local-private |
| M-218 | work/native-object-admission-reproduction-20261014/validation.json | 728f7a86a574e60e21a714cea41884773f83f8ff430f4eab20896b250b666212 | 2026-10-13 | Before/after native object cases,55tests and six release cases;100k22-generation benchmark with exact state and retained failures | local-private |
| M-219 | work/native-object-admission-fix-review-20261014/tests-serialization-verdict.md | c45b5370d5c37144176161f45851ffa2c932426da0131a89047cf3284353ffb2 | 2026-10-13 | Fixture serialization delta review;11checks preserve all assertions and production source; failure mechanism not independently observed | local-private |
| M-220 | work/native-object-admission-reproduction-20261014/validation-final.json | d74ea9a02b33f5d2a4f85fd71781c4e2a514235ed794b8f9a7cd8f9cc823b792 | 2026-10-13 | Final full manual suite explicitexit0; unchanged production/runtime joins, current test review and prior failed suite retained | local-private |
