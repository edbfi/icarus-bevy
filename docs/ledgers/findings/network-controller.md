# Findings: Network controller

Connected startup observations and controller receive boundaries.
[Interpretation](../../runbooks/controller-receive.md).

Last reconciled: 2026-11-17

| ID | Claim | Kind | Evidence | Scope and limits | Unknowns | Status |
| --- | --- | --- | --- | --- | --- | --- |
| F-1184 | Protected stalled client yields actor78 recognized, actor82 ignored, Begin slots/profile requests 0 and one skipped unknown-subobject block despite settled opens/possession | measured | live; M-1417/ev_4cd2d52e9a39, M-1418; 8f51a672 | high; one protected run; Begin absence inconclusive; no world entry/movement/checkpoint proof; native exit unproved | U-038 | active |
| F-1185 | Protected production run accepts Begin/profiles/finalisation and releases pawn; owner enters, walks, looks, sprints and jumps after leaving mountain spawn | observed | live/user-report; M-1421/ev_73c1f93e4719, M-1422, M-1423; 776f0c3 | one account/run; restart plus authored speeds; placement/audio/collision unqualified; no causal isolation | U-038, U-080 | active |
| F-1186 | Client leave shows saving pending; Return to Title Screen returns to gameplay; owner uses Alt+F4. Native saves seven foundation generations and exits 0 | observed | live/user-report; M-1421/ev_73c1f93e4719, M-1422, M-1423; 776f0c3 | one run; player persistence off; ignored actor149/169 identities unbound; no normal-leave/player-save proof | U-038 | active |
| F-1199 | Checkpoint-enabled protected join stalls at Preparing Geometry without accepted Begin/profile; 210s timeout ends phase, owner reports error 62 | observed | live/user-report; M-1511, M-1512/ev_9aaf970d525a; 050d69c | one 3.0.36 join, zero completed phases; error causality, world entry, leave/restart unproved; cleanup separately verified | U-038 | active |
| F-1201 | Checkpoint mode holds the pawn for Begin/profile while restart-pawn needs it settled: a peer sending Begin only after ClientRestart gets no pawn or field 40. Opt-in `checkpoint_pawn_before_identity` opens it first; stored vitals precede the record | measured | authored-test; M-1524; e9a3615 | high; 16 tests; location unapplied, proof refused; no client proof | U-038 | active |
| F-1203 | `status`/`stopped` `pawn_receive` keeps the first 64 pawn-channel field keys since bind (target, index, reliability, disposition; size range), movement apart, sub-objects as own-open ordinals; classes lacking a field bound and unframed bunches are only counted | measured | authored-test; M-1527; 6604142 | high; 13 tests; numbers/enums only; no call identity proof | U-038 | active |
| F-1204 | 26613688 `ClientMoveResponsePacked` correction: clear ack bit, f32 TimeStamp, base/rotation/montage/source flags, NewLoc and NewVel 3x f32, optional rotation, base, bone, mode, base-relative; packed count <=2048; same as 26425113 | derived | static; M-1528/ev_3cda830d31da, M-1528/ev_eba251b382fd, M-1530 | high; instruction level; source group, TimeStamp origin open | U-038 | active |
| F-1205 | Native move-response codec (strict reader, correction writer) re-encodes S2's 1,340 acks and 20 corrections bit for bit; source-flag corrections are refused; nothing sends a correction | measured | capture, authored-test; M-1529; 0633e59 | high; one 3.0.34 session, little order; no client effect shown | U-038 | active |
