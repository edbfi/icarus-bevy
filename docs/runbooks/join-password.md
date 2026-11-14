# Join passwords

## Configuration and listing

F-1161 to F-1163, F-1167. Set `[network] join_password` for the native server;
unset or empty disables password protection. Without `[network.hosting]`,
`run` permits only loopback and publishes no Steam listing. Explicit hosting
can bind publicly; [connected-run gates](../gates.md#before-any-original-server-or-client-run)
apply, including the owner's word before a public passwordless launch.

The probe accepts `--join-password-file work/<case>/join-password`: a regular
UTF-8 file under the checkout's `work/`, without symlinks or traversal, mode
0600 and at most 1 KiB. It trims one trailing newline and refuses empty content.
Both forms allow at most 64 UTF-16 units, no control characters or `?`.

Protected hosted/probe listings add `L_s`, which makes the browser prompt.
Login compares the URL `password` option ignoring ASCII case; a missing option
is empty. A mismatch receives `Invalid Password` before capacity checks.

## Privacy and observations

Passwords are never logged, serialized or echoed. `check-config` omits the
password; records and probe lines expose only the `join_password` boolean.
Configuration errors report the rule and position, not the password text.

One protected stock-client run observed wrong-password denial and
correct-password admission (F-1181). World loading then stalled before recorded
Begin; that run does not qualify world entry, movement or checkpoints (U-038).
