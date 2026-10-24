# Security

## Reporting a vulnerability

Please report vulnerabilities privately through
[GitHub's private vulnerability reporting](https://github.com/Vironnimo/vEdit/security/advisories/new),
not in a public issue. Include the vEdit version, what an attacker can do and
the steps to reproduce it. You will get an answer within a week.

## Scope

vEdit is a local desktop editor. Its control CLI connects over loopback with a
per-session token stored in the user profile, so any program running under the
same Windows account can control the editor. That is by design, as described in
the README's Security section. Reports about reaching the control connection
from another account or another machine, about the editor writing files it
should not, or about installer and update tooling are in scope.

Only the latest release receives fixes.
