---
name: Commander notifications
description: Shared native toast feedback within Commander's existing Carelo appearance.
colors:
  carelo-toolbar: "#202227"
  carelo-text: "#e7e9ee"
  carelo-muted: "#b0b6c0"
  carelo-accent: "#528bff"
  notification-error: "#da5b62"
typography:
  body:
    fontFamily: 'Inter, Cantarell, sans-serif'
    fontSize: "13px"
rounded:
  toast: "8px"
  toast-button: "4px"
spacing:
  toast-margin: "12px"
  content-gap: "10px"
  content-block-padding: "4px"
components:
  toast:
    backgroundColor: "{colors.carelo-toolbar}"
    textColor: "{colors.carelo-text}"
    rounded: "{rounded.toast}"
  toast-button:
    rounded: "{rounded.toast-button}"
---

# Design System: Commander notifications

## Overview

This scope documents transient feedback in the native GTK4/libadwaita application.
Compact toasts float above the working surface without adding inline error or
information rows. They inherit Commander's Carelo palette and bundled Lucide icons.

The implementation authority is [notifications.rs](../../crates/app/src/app/notifications.rs)
and the notification rules in [style.css](../../crates/app/assets/style.css).
This extends the existing appearance; it does not define a new application identity.

## Colors

The Carelo values above record the built-in dark defaults. Runtime appearance and
desktop palettes remain authoritative for the toolbar surface, foreground, muted
information icon, and accent success icon. The error triangle uses the fixed error
color. Icon shape and accessible severity labels distinguish all three kinds.
The fine border follows the foreground through `carelo_separator` at 8% opacity.

## Typography

Toast copy uses the application sans-serif body type. Render messages as plain text,
including filenames containing markup characters. Wrap at words or characters for
long paths and Unicode; show up to three lines with an ellipsis and a maximum width
of 58 characters. The full message remains in the tooltip.

## Layout

The main window and utility dialogs contain native toast overlays. Custom dialogs
receive an overlay when needed. Feedback appears near the bottom of its active
surface; `success()` targets the main window when a completed dialog has closed.
The icon is 18px, beside the wrapping message. Native libadwaita controls the action,
dismissal, placement, and animation; narrow dialogs keep their own visible toast.

## Elevation & Depth

The toolbar-colored surface, fine border, and shadow (`0 4px 14px` with black at
22% opacity) separate transient feedback from the content below. Toast buttons have
no shadow and retain the app's visible keyboard focus treatment.

## Shapes

Use the toast and toast-button corner sizes above. Severity changes the icon and
its color while retaining the same compact surface and message layout.

## Components

- Errors have high priority and a 10-second timeout. Information and success use
  5 seconds; explicit actions use 10 seconds. Native dismissal remains available.
- At most four toasts remain pending across the app. New traffic evicts a lower
  priority toast first. Information cannot evict a queue containing only errors;
  a new error can replace the oldest error when all four are errors.
- Suppress the same kind and message for two seconds on the same overlay. Model
  observation and dialog feedback state also prevent unchanged errors from
  replaying during redraws, resizing, progress ticks, or repeated validation.
- Errors offer **Copy** unless they have an explicit action. Information and
  success offer **Copy** for text longer than 160 characters or containing a
  newline. Copy preserves the full trimmed message. Custom action removal offers
  **Undo**; remote connection failures offer **Options** to open recovery choices.
- Field validation waits 650ms and only emits the latest message while the field
  remains mapped. Its tooltip and accessible Description retain the current
  diagnostic after dismissal and clear when the field becomes valid. Toast labels
  include an accessible Information, Completed, or Error prefix.

## Do's and Don'ts

- Do route transient errors, information, and completion feedback through the
  shared notification helpers, including dialog feedback.
- Do keep normal counts, progress, metadata, static field instructions, durable
  reports, and safety confirmations in their existing content surfaces.
- Don't add inline error or information rows, or open a modal merely to report a
  transient failure.
- Don't use toast dismissal as a reason to erase invalid-field diagnostics or
  durable operation records.
