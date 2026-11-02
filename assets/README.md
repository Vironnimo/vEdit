# Application icon

The vEdit icon is the user-selected **08 - Discover** artwork from
`v-family-2026-10-22/explorations/08-discover`.

The motif is cut out of the artwork's tile and placed on vBot's tile at vBot's
mark size, so the app icons of vBot, vWatch and vEdit read as one family in the
taskbar.

- `vedit.ico`: 16, 20, 24, 32, 40, 48, 64, 96, 128 and 256 pixel variants for
  Windows executable resources and Shell icons.
- `vedit.png`: the 256 pixel tile, embedded in the running window and displayed
  in the repository README.
- `vedit-mark.png`: the transparent 256 pixel mark without tile, shown in the
  custom titlebar like `vbot-mark-transparent.png` in vBot's header.

Keep all three files from the same export. None of them requires the original
asset directory at build time or runtime. The portable package includes both
formats; shortcuts and Windows registration refer to the executable resource.

After copying replacements, set their modification times to the current time.
Windows `Copy-Item` preserves source timestamps; an icon older than the previous
build can leave Cargo reusing the old executable resources. Run
`scripts/test-branding.ps1` against the finished package to check the actual icons.
