/** @type {import('tailwindcss').Config} */
module.exports = {
  content: ["./templates/**/*.html", "./src/**/*.rs", "./assets/**/*.js", "!./assets/vendor/**"],
  // LC-190: theme is driven by `<html data-theme="...">` (set no-flash by the
  // bootstrap in LC-191). `dark:` variants key off the dark theme; the primary
  // mechanism is the token utilities below, which recolor via CSS vars.
  darkMode: ["selector", '[data-theme="dark"]'],
  theme: {
    extend: {
      // LC-190: semantic design tokens -> Tailwind color utilities, mapped to
      // the CSS vars in assets/main.css so `bg-surface`, `text-content`,
      // `border-border`, `bg-accent text-accent-content`, etc. recolor per
      // `data-theme`. `extend` (not override) keeps the raw slate/white
      // palette available for templates not yet migrated (LC-193).
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
        },
        warning: {
          DEFAULT: "var(--warning)",
          content: "var(--warning-content)",
        },
        danger: {
          DEFAULT: "var(--danger)",
          content: "var(--danger-content)",
          // LC-215: soft-danger callout. `bg-danger-surface` for the
          // panel bg, `border-danger-border` for the outline. Heading +
          // body copy on these panels use `text-danger`.
          surface: "var(--danger-surface)",
          border: "var(--danger-border)",
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

