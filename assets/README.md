# Application icon

The vEdit icon is the user-selected **08 - Discover** artwork from
`v-family-2026-10-22/explorations/08-discover`.

- `vedit.ico` is the unchanged `icon.ico`: 16, 20, 24, 32, 40, 48, 64, 96,
  128 and 256 pixel variants for Windows executable resources and Shell icons.
- `vedit.png` is the unchanged `icon-256.png`, embedded in the running window
  and custom titlebar and displayed in the repository README.

Keep both files from the same artwork export. Neither requires the original
asset directory at build time or runtime. The portable package includes both
formats; shortcuts and Windows registration refer to the executable resource.

After copying replacements, set their modification times to the current time.
Windows `Copy-Item` preserves source timestamps; an icon older than the previous
build can leave Cargo reusing the old executable resources. Run
`scripts/test-branding.ps1` against the finished package to check the actual icons.
