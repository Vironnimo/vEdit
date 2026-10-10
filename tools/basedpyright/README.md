# Portable Python language support

vEdit's release includes basedpyright 1.40.1 and the build machine's Node runtime.
The pinned package and lockfile make the language server installation reproducible.
`scripts/package-language-tools.ps1` installs it locally with npm lifecycle scripts
disabled, then copies it into `dist/vEdit/tools`. No global Python or npm packages
are changed. The Node version and its matching license are included beside node.exe;
basedpyright's license remains inside its package.

Python analysis discovers a project-local or PATH language server first, then uses
the bundled server. Select the project's Python interpreter in Settings so imports
resolve against the intended environment. Python itself remains the user's chosen
runtime; it is not bundled or replaced.

Upstream: [basedpyright installation](https://docs.basedpyright.com/latest/installation/command-line-and-language-server/),
[Node.js](https://nodejs.org/).
