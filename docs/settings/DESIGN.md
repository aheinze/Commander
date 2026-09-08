---
name: Commander Settings
description: Implemented native settings surface; scoped to this dialog and shortcut capture.
colors:
  carelo-accent: "#528bff"
  carelo-toolbar-dark: "#202227"
  carelo-text-dark: "#e7e9ee"
  carelo-muted-dark: "#b0b6c0"
  carelo-toolbar-light: "#ffffff"
  carelo-text-light: "#252a33"
  carelo-muted-light: "#59616d"
typography:
  body: { fontFamily: "Inter, Cantarell, sans-serif", fontSize: "13px" }
  row-title: { fontSize: "13px", fontWeight: 500 }
  section: { fontSize: "13px", fontWeight: 600 }
  description: { fontSize: "12px", lineHeight: 1.4 }
  action: { fontSize: "12.5px", fontWeight: 600 }
rounded:
  action: "4px"
  navigation-row: "5px"
spacing:
  label-gap: "4px"
  row-gap: "16px"
  page-inset: "24px"
---

# Design System: Commander Settings

## Overview
This record covers Settings and its shortcut recorder only. Both extend the existing GTK4/libadwaita utility dialog: compact text, flat form rows, a native sidebar, and native controls. The five pages are Workflow, Keyboard, Appearance, History, and About.
Implementation sources: [settings.rs](../../crates/app/src/app/settings.rs), [dialogs.rs](../../crates/app/src/app/dialogs.rs), [style.css](../../crates/app/assets/style.css), and [theme.rs](../../crates/app/src/app/theme.rs).

## Colors
The frontmatter records the built-in Carelo values; Midnight Blue, Forest, Aubergine, and automatic Omarchy palettes can replace them at runtime. Light appearance uses the light text and surface tokens. Dividers derive from foreground at 8% opacity.
Settings links mix the active accent toward foreground by 45%. Apply and Assign use white text over accent mixed toward black by 60%, or 55% on hover. Preserve these scoped contrast treatments across appearances and custom palettes.

## Typography
Body text uses the inherited Inter/Cantarell stack. Native title styles distinguish page headings and the About identity; smaller descriptions wrap beneath row titles. Shortcut labels use 12px text; the recorder uses 20px text. The dialog title uses 13.5px, weight 600.

## Layout
Settings requests 780 × 680 logical pixels; the sidebar requests 145px width. Pages scroll vertically with no horizontal scrollbar, using 18px top padding and the page inset on the other edges. Labels expand beside vertically centered controls; descriptions wrap.
Rows use 13px vertical padding, reduced to 9px in shortcut results. Sections have 22px top spacing. Cancel and Apply remain in a separate right-aligned footer. The recorder requests 440 × 300 logical pixels. A 620px-wide Settings capture records the compact layout.

## Elevation & Depth
Content uses a shared dialog surface, transparent lists, and single quiet separators. Header and footer buttons remove box shadows; libadwaita supplies the floating dialog shell. Footer hover transitions take 90ms with ease-out timing.

## Shapes
Footer actions and sidebar selection use the documented small radii. Switches, dropdowns, spin controls, search entry, and outer dialog retain their native widget shapes.
The header close button shares the main window's `chrome::close_button`: a 24px flat control, 14px X icon, 4px radius, and the same hover and focus treatment. It respects the dialog's close permission.

## Components
- Workflow groups startup restoration, folder ordering, archive browsing, transfers, and access to context-menu tools. Appearance holds theme choices and inspector work controls. History exposes retention, a 5–100 limit, and a clear-on-Apply state.
- Keyboard focuses search on entry and filters action names, command IDs, and shortcut labels. Classic and Modern profiles retain separate overrides. The recorder names shortcut conflicts before Assign moves a binding; Enter assigns, Tab navigates, and Escape cancels. Remove and profile reset update the draft.
- Apply commits preference and shortcut drafts; Cancel discards them. Context-menu tool management and About actions are separate immediate actions. About displays the existing app icon, version, creator profile, license, platform, runtime, project links, and copyable diagnostic details. Its update controls check stable releases on request, show release notes, and save verified downloads for manual installation. Closing the dialog cancels unfinished update work.

## Do's and Don'ts
- Do preserve native keyboard navigation, accessible control labels, wrapping descriptions, staged preference changes, and visible conflict explanations.
- Do check light and dark appearances, keyboard capture, and compact width against [settings tests](../../crates/app/src/app/settings/tests.rs). Captures are under `target/settings-native-contrast/app-settings-tests-gtk_settings_shortcuts_preferences_about_and_persistence/snapshots/`.
- Don't promote these settings-specific dimensions or page arrangements into a project-wide design system.
