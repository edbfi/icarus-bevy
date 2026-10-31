# Findings: Network presence

How the original server advertises itself to Steam and its server browser, and
how clients choose a connection route. IDs remain in the shared finding space.

Before restructure: ab9f866:docs/ledgers/findings/network.md

Last reconciled: 2026-10-31

| ID | Claim | Kind | Evidence | Scope and limits | Unknowns | Status |
| --- | --- | --- | --- | --- | --- | --- |
| F-1002 | 3.0.34 answers A2S as protocol 17, game Icarus, app 0, type d, OS l, public despite a password, VAC 1, version 0.0.0.1, game ID 1149460, with session keywords and 11 rules (CONMETHOD, P2PADDR, P2PPORT, G_s...); Steam lists gamedata, gametype, secure | observed | runtime-capture; M-1153 | high; 2 no-player Proton runs; OS l may come from the Linux steamclient | U-078 | active |
| F-1003 | 3.0.34 maps CONMETHOD None/Direct/P2P/PartnerHosted to 0..3 and sets 1 + its Steam networking flag; clients use the host address only for Direct, else steam.<P2PADDR>; flags off, it advertises Direct | observed | static-bytes, runtime-capture; M-1154, M-1153 | high; resolver read in the server binary, client inferred; no join | U-038, U-078 | active |
| F-1004 | An anonymous native game-server logon with GSServerType and CMsgGameServerData gets a secure status reply; Steam then lists the native advert with F-1002's fields and CONMETHOD Direct, its A2S port answers behind challenges, and logoff delists it | measured | runtime-capture, authored-test; M-1156 | high; one 120 s probe, no game port or client | U-078 | active |
