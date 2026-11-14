# Findings: Network controller

Connected startup observations and controller receive boundaries.
[Interpretation](../../runbooks/controller-receive.md).

Last reconciled: 2026-11-14

| ID | Claim | Kind | Evidence | Scope and limits | Unknowns | Status |
| --- | --- | --- | --- | --- | --- | --- |
| F-1184 | Protected stalled client yields actor78 recognized, actor82 ignored, Begin slots/profile requests 0 and one skipped unknown-subobject block despite settled opens/possession | measured | live; M-1417/ev_4cd2d52e9a39, M-1418; 8f51a672 | high; one protected run; Begin absence inconclusive; no world entry/movement/checkpoint proof; native exit unproved | U-038 | active |
| F-1185 | Protected production run accepts Begin/profiles/finalisation and releases pawn; owner enters, walks, looks, sprints and jumps after leaving mountain spawn | observed | live/user-report; M-1421/ev_73c1f93e4719, M-1422, M-1423; 776f0c3 | one account/run; restart plus authored speeds; placement/audio/collision unqualified; no causal isolation | U-038, U-080 | active |
| F-1186 | Client leave shows saving pending; Return to Title Screen returns to gameplay; owner uses Alt+F4. Native saves seven foundation generations and exits 0 | observed | live/user-report; M-1421/ev_73c1f93e4719, M-1422, M-1423; 776f0c3 | one run; player persistence off; ignored actor149/169 identities unbound; no normal-leave/player-save proof | U-038 | active |
