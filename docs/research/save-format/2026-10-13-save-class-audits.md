# Supplied-save class audits C009, C028, C034, D01 and DA04

Scope: structural qualification of selected supplied-save classes (C009 with its D01 nested arrays and DA01-DA04 families, C028, C034, plus C013/C001 census status) against both supplied copies, and the archive-reader changes (struct-array limits, borrowed strict string frames) that enabled them; build/revision not stated beyond library pins.
Full text: fabd2f9:docs/implementation-status.md lines 258-279, 303-328, 350-376, 394-434, 457-476, 490-628, 641-710, 796-884.

## Question cluster

- Which selected save classes have exact, individually qualified header, width and string-frame structure in both supplied copies (F-115, F-119)?
- Which descriptor families lack populated coverage (U-021)?
- What remains outside the selection, and what is still unknown about meanings, restoration and original behavior (U-020, U-022, U-025, U-026)?

## Answered

- C028 selection: 29,762 top and 20,704 direct-child fields across 39 descriptors; no observed populated structure gap remains (F-115, F-117).
- C028 top: 6,470 Name/Enum/Str frames agree native vs strict (F-135); 3,882 Int4 and 2,588 Bool0 ranges bounded (F-137, F-138).
- C028 Struct children: 7,764 payloads and 20,704 child headers qualify (F-128, F-129, F-131); leaves qualify as Int4/UInt32/Bool0, 16/12/12-byte binary Structs and nine-byte Name frames (F-123, F-124, F-125).
- C034: top (F-146, F-147), own (F-150) and inherited (F-152) descriptors qualify by exact identity.
- C009 top: 3,402 string spans, 1,458 Int and 486 Bool spans qualify via execution-v3 (F-182, F-183).
- C009 common children: 9,018 headers (F-174, F-175, F-176), 1,468 binary-width spans (F-172, F-173) and 7,272 scalar spans (F-169, F-170) qualify.
- C009 deeper arrays / D01: ND01 headers (F-165, F-166, F-167), D01 leaves (F-162), nested headers (F-158, F-159, F-160) and nested leaves (F-155) qualify.
- D01 Array getter/schema/ops chains already resolved in retained static evidence (F-164).
- C013 singleton bodies already have exact structure and native top aggregates (F-120).
- Struct-array reader accepts explicit bounds up to a 128 KiB research ceiling while the original API keeps 64 KiB (F-178, F-179).
- Borrowed `read_string_frame` removes success-path allocation with unchanged strict behavior (F-142, F-143, F-145).
- Native save/restart smokes pass for both reader changes (F-144, F-179).
- Toolchain and direct dependency pins confirmed current (F-185).

## Partial

- Census: 3,440 of 5,485 records covered by C009/C013/C028/C034 selections (F-119, U-022).
- C001: four records with retained parent metadata; native class getter and construction callback identified, bridge unqualified (F-121, F-122, U-023).
- C028 historical top native batch gives aggregates only; new top ranges are JS observations (F-140, F-141, U-024).
- Optional descriptors all present in supplied copies, not proven mandatory elsewhere (F-130).
- C034 Enum shares reviewed Name/FString transfer; storage is not an archive width (F-148).

## Unresolved

- Meanings, Name identity and Enum lookup for all qualified values (U-020).
- Empty families: 17 C028 array-child descriptors, DA04's two descriptors, CA06 Text/Name (U-021; F-116, F-156, F-169, F-176).
- 34 classes / 2,045 records outside selection (U-022).
- C001 construction/vtable/archive-dispatch bridge (U-023).
- C028 individual native top spans and historical library snapshot (U-024).
- Official restoration/import, client and newer-build compatibility (U-025).
- Original normalization/EOF/error equivalence (U-026).
- Opaque C028 binary Struct bodies and grandchildren (U-027).
- Original performance comparison (U-028).

## Limitations and exclusions

- Counts and maxima are samples from two supplied copies, not format limits (F-160, F-166, F-176).
- No decoded units, text or numerical values were emitted or interpreted in any case (F-155, F-162, F-170).
- Helper/reader caps unchanged: 7-field post-parse, 4,096-tag, 4,096-element/256-field, 1,701-frame (F-134, F-157, F-161, F-167).
- D01 header case does not claim the protocol harness's process-identity cleanup proof (F-161).
- Earlier array-limit smoke against the previous executable excluded from release result (F-179).
- Defects caught before execution: C034 TDZ, C009 receipt ordering, post-hashing deadline (F-151, F-171, F-181); v2 retry could not execute (F-180).
- A worker's fixity-byte read beyond its delegation boundary is recorded (F-154).
- crates.io JSON endpoints returned HTTP 403 (F-185).

## Receipts

- Coverage audit and census map: M-047, M-048.
- C028: M-049..M-055.
- Borrowed frames: M-056, M-057.
- C034: M-058..M-061.
- D01: M-062..M-065.
- C009: M-066..M-070, M-075.
- Array limits and prechange snapshots: M-071..M-074.
- Dependency recheck: M-076.
- Binding/fixity receipts per case: F-126, F-132, F-136, F-139, F-149, F-153, F-157, F-161, F-163, F-168, F-171, F-173, F-177, F-183, F-184.
