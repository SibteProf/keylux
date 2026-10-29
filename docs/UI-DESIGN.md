# Keylux desktop design

The interface is a lighting studio for keyboard owners. Its defining element is
the physical keyboard canvas: lighting provides the colour while navigation and
controls remain quiet. The existing Rust/egui desktop stack and the Play, Create,
Apps, and Settings destinations are retained.

## Audit and decisions

The previous interface gave the effect library permanent space on every screen,
mixed connection state with frame counters, and placed the canvas below an
increasing number of layer controls. Small buttons, mixed emoji, and long rows
made the layout harder to scan and resize.

- Stable navigation now occupies a hideable sidebar. The library appears only
  in Play and supports search. Compact layouts retain effect selection, import,
  refresh, and folder access without requiring the sidebar.
- Connection state and pause/resume stay in the toolbar. Device diagnostics move
  to a contextual menu. Errors appear near the task instead of at the window's
  bottom edge.
- Create uses a layer inspector beside the keyboard at regular widths. Compact
  layouts put the canvas first and disclose the layer inspector underneath.
  Brush colour and brush mode stay beside the canvas; additional paint and
  animation controls are grouped below.
- Settings groups appearance, keyboard connection, and window behaviour.
  OpenRGB connection details are disclosed separately. Apps groups default
  lighting and application profiles, with guidance when no profiles exist.
- Labelled text buttons replace mixed emoji. The editor keeps direct painting
  and also offers keyboard-operated key selection, inclusion, exclusion, and
  painting. Standard egui widgets expose accessibility metadata through AccessKit.
- Layers use full-width selectable rows with their type and visibility together.
  Ordering controls appear only for multiple layers, with the same actions in a
  row's context menu. Document name and save status form one group; Save and File
  contain document actions. Live preview is a checkbox on the keyboard canvas.

## Visual system

This is an egui implementation informed by desktop HIG principles, rather than a
claim to use native Apple components or Liquid Glass. The frontend design skill's
marketing layouts do not apply to this product UI. Its brief inference,
audit-first approach, consistency rules, and restraint do apply. The design dials
are variance 5, motion 1, density 4: a familiar utility layout, interaction feedback
only, and sufficient space to work with layers.

Platform UI fonts are used when installed, with egui fallbacks. Body and button
text use 14 logical points, supporting text 12, section titles 17, and page titles
25. Controls have a default 32-point interaction height. Interface scaling is
available from 80% to 200%. Surfaces use 10-point corners; controls use 6; keycaps
use 4. There is no decorative motion or transparency.

| Role | Dark | Light |
| --- | --- | --- |
| Workspace | `#14161a` | `#f1f3f7` |
| Surface | `#1d2026` | `#fcfdff` |
| Content | `#eff2f7` | `#1e2531` |
| Secondary content | `#aab4c3` | `#556376` |
| Accent text | `#91c0ff` | `#1f5399` |
| Primary action | `#2b5b97` | `#1f5399` |

Body/surface contrast is 14.54:1 in dark appearance and 15.12:1 in light;
secondary text is 7.79:1 and 6.01:1. White primary-action labels are 6.89:1 and
7.62:1. Regression tests also check text against alternate surfaces. LED key
labels choose black or white from linear sRGB luminance rather than a simple
brightness threshold.

## Guidance consulted

The bundled apple-design references supplied these principles:

- `layout.md`, **Visual hierarchy**: “Group related items to clearly express
  related information or functions.” This informs the inspector and form groups.
- `layout.md`, **Adaptability**: retain the same functionality as available space
  changes; reorganize rather than discard controls.
- `sidebars.md`, **Best practices**: support showing and hiding the sidebar and
  keep its hierarchy shallow.
- `toolbars.md`, **Best practices**: “Choose items deliberately to avoid
  overcrowding.” This informs the device menu and contextual actions.
- `typography.md`, **Ensuring legibility** and **Conveying hierarchy**: use readable
  sizes and a restrained scale.
- `color.md`, **Inclusive color**, and `accessibility.md`, **Vision**: use contrast
  measurements, text alternatives, and adjustable interface size.
- `settings.md`, **General settings** and **Task-specific options**: keep general
  preferences in Settings and editing controls with the canvas.
- `buttons.md`, **Style**: reserve prominent treatment for the main action.

The desktop and cross-platform reference files inform translation into egui;
Apple-specific platform conventions are not imposed on Windows or Linux.

## Verification

Automated checks cover light/dark text contrast and editor layout at compact,
regular, and wide workspace widths, including larger text, many layers, motion
editing, and key selection. The running Windows app was inspected in both themes,
and its accessibility tree was checked. Full screen-reader certification and
visual verification on macOS and Linux remain outside this pass.

Shortcuts: Ctrl/Cmd+1 through 4 navigate the four destinations; Ctrl/Cmd+comma
opens Settings; Ctrl/Cmd+S saves the composition; Ctrl/Cmd+Z and redo shortcuts
operate on editor history.
