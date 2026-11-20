# Logo

Three concepts, all drawn from the same 16-unit grid so the one-colour panel icon is pixel-aligned at
16 px. `sheet.png` shows each at 512, 128, 32 and 16 px on light and dark, the 16 and 32 px tray cut
enlarged, and the one-colour icon in the panel's text colour.

| Concept | Idea | Files |
| --- | --- | --- |
| **links** (in use) | A phone and a computer screen interlocked like two links of a chain | `links.svg`, `links-symbolic.svg` |
| orbit | A phone with a tether looped around it, passing in front low and behind high | `orbit.svg`, `orbit-symbolic.svg` |
| leash | A phone outline on a cord whose far end is the computer | `leash.svg`, `leash-symbolic.svg` |

Why links: it is the plainest picture of what Tether does (a phone joined to a computer), its two
different rectangles give it a silhouette no other app has, and it stays readable at 16 px.

## Where the icon lives

- `crates/tether-app/icons/icon.svg`: the app icon; `icon.png` (512), `128x128.png` and `32x32.png` are
  renders of it for Tauri, the Flatpak and install.sh.
- `crates/tether-app/icons/tray.png`: the tray cut (no margin, larger mark), which the tray scales and
  adds the unread badge to.
- `crates/tether-app/icons/icon-symbolic.svg`: the one-colour icon for GNOME and KDE panels (fills only,
  so GTK can recolour it).

## Regenerating

`gen.py` holds the geometry. To change a concept or switch the app to another one:

```sh
cd docs/logo
uv run --no-project --with cairosvg --with shapely --with pillow python gen.py .
```

That rewrites the SVGs and `sheet.png` here. To apply a concept, render it into
`crates/tether-app/icons/` with `app_svg(name)` for `icon.svg`/`icon.png`/`128x128.png`/`32x32.png`,
`app_svg(name, small=True)` at 256 px for `tray.png`, and `symbolic_svg(name)` for `icon-symbolic.svg`.
