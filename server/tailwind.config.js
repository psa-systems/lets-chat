/** @type {import('tailwindcss').Config} */
module.exports = {
  content: ["./templates/**/*.html", "./src/**/*.rs", "./assets/**/*.js", "!./assets/vendor/**"],
  // LC-190/LC-541: two independent axes on `<html>`, set no-flash by the
  // bootstrap in LC-191/LC-541. `data-mode` (light/dark/hc-light/hc-dark)
  // drives `dark:` variants below - `$=` (ends-with) so `hc-dark` matches
  // too, not just the literal `dark`; `data-theme` selects the color
  // palette. The primary mechanism is the token utilities below, which
  // recolor via CSS vars.
  darkMode: ["selector", '[data-mode$="dark"]'],
  theme: {
    extend: {
      // LC-190: semantic design tokens -> Tailwind color utilities, mapped to
      // the CSS vars in assets/main.css so `bg-surface`, `text-content`,
      // `border-border`, `bg-accent text-accent-content`, etc. recolor per
      // `data-mode` (light/dark/hc-light/hc-dark) and `data-theme` (the
      // active color palette). `extend` (not override) keeps the raw
      // slate/white palette available for templates not yet migrated (LC-193).
      colors: {
        surface: {
          DEFAULT: "var(--surface)",
          elevated: "var(--surface-elevated)",
          sunken: "var(--surface-sunken)",
        },
        content: {
          DEFAULT: "var(--content)",
          muted: "var(--content-muted)",
          subtle: "var(--content-subtle)",
        },
        border: {
          DEFAULT: "var(--border)",
          strong: "var(--border-strong)",
        },
        accent: {
          DEFAULT: "var(--accent)",
          hover: "var(--accent-hover)",
          content: "var(--accent-content)",
          // LC-222: tinted accent surface. `bg-accent-surface` for active
          // sidebar rows, mention popover avatar, voted poll bar, settings
          // device chip, reply-count chip. Soft fill that themes per palette.
          // `text-accent-surface-content` is the matching foreground (light
          // themes: --accent for blue affordance; dark themes: --content for
          // readability against the dim blue-900 surface).
          surface: "var(--accent-surface)",
          "surface-content": "var(--accent-surface-content)",
        },
        success: {
          DEFAULT: "var(--success)",
          content: "var(--success-content)",
          // LC-224: soft-success callout (saved banners, healthy bridge
          // chip, created-token blocks, public-enclave badge). Foreground
          // pair `text-success-surface-content` themes per palette.
          surface: "var(--success-surface)",
          border: "var(--success-border)",
          "surface-content": "var(--success-surface-content)",
        },
        warning: {
          DEFAULT: "var(--warning)",
          content: "var(--warning-content)",
          // LC-224: soft-warning callout (missing-secret banners, pinned
          // strip, stale bridge chip, unverified-email chip). Foreground
          // pair `text-warning-surface-content` themes per palette.
          surface: "var(--warning-surface)",
          border: "var(--warning-border)",
          "surface-content": "var(--warning-surface-content)",
        },
        // LC-750: the sidebar star toggle, the one sidebar glyph that used to
        // be a fixed amber and so never moved with the palette. Its own token
        // rather than `warning`, which means something else.
        star: "var(--star)",
        danger: {
          DEFAULT: "var(--danger)",
          content: "var(--danger-content)",
          // LC-215: soft-danger callout. `bg-danger-surface` for the
          // panel bg, `border-danger-border` for the outline. Heading +
          // body copy on these panels use `text-danger`.
          surface: "var(--danger-surface)",
          border: "var(--danger-border)",
        },
        // LC-225: message-actor badges (webhook / email / bridge). Each
        // kind has a soft surface + matching content text color so the
        // three badges dim together in dark themes instead of staying
        // bright. Per-hue identity preserved.
        webhook: {
          surface: "var(--webhook-surface)",
          content: "var(--webhook-content)",
        },
        email: {
          surface: "var(--email-surface)",
          content: "var(--email-content)",
        },
        bridge: {
          surface: "var(--bridge-surface)",
          content: "var(--bridge-content)",
        },
      },
      ringColor: {
        DEFAULT: "var(--ring)",
        // LC-222: explicit `ring-ring` / `focus:ring-ring` named alias.
        // Tailwind's `ring-DEFAULT` resolves only when the class is
        // literally `ring` (no suffix); to use it inside an explicit
        // `ring-{size}` pair the color needs its own named key.
        ring: "var(--ring)",
      },
    },
  },
  plugins: [],
}

