import { heroui } from "@heroui/react";

/** @type {import('tailwindcss').Config} */
export default {
  content: [
    "./index.html",
    "./src/**/*.{js,ts,jsx,tsx}",
    "./node_modules/@heroui/theme/dist/**/*.{js,ts,jsx,tsx}",
  ],
  theme: {
    extend: {
      fontFamily: {
        mono: [
          "Agave",
          "ui-monospace",
          "SFMono-Regular",
          "Menlo",
          "Monaco",
          "monospace",
        ],
      },
    },
  },
  darkMode: "class",
  plugins: [
    heroui({
      themes: {
        light: {
          colors: {
            primary: {
              DEFAULT: "#5b6cff",
              foreground: "#ffffff",
            },
            focus: "#5b6cff",
          },
        },
        // VSCode's "Night Rider" (trustfall/vscode-night-rider), mapped onto
        // HeroUI's slots from the theme's own JSON — editor.background,
        // sideBar/widget backgrounds, the token palette's purple/pink/blue —
        // not eyeballed approximations. The default theme; "dark" (below)
        // remains and is one palette-command away.
        "night-rider": {
          extend: "dark",
          colors: {
            background: "#1e1c3f",
            // The `default` ramp drives inputs, hovers and subtle borders in
            // HeroUI — without restyling it those stay the old neutral gray
            // and clash with the purple surfaces. A violet-tinted ramp built
            // between the theme's own backgrounds and foregrounds.
            default: {
              50: "#1A1837",
              100: "#222246",
              200: "#2D2B55",
              300: "#38356A",
              400: "#454180",
              500: "#5D5988",
              600: "#696292",
              700: "#8481B5",
              800: "#A5A2CC",
              900: "#C9CBDB",
              DEFAULT: "#2D2B55",
              foreground: {
              // The scale inverts in dark themes (50 darkest → 900 lightest),
              // and the inherited ramp's mid-steps — the ones muted text uses
              // (`foreground-500/600`) — sat too dark against purple. Lifted
              // so secondary text stays legible without shouting.
              50: "#2D2B55",
              100: "#38356A",
              200: "#454180",
              300: "#5D5988",
              400: "#8481B5",
              500: "#9B98C6",
              600: "#A9A6D1",
              700: "#B8B6DB",
              800: "#C4C3E0",
              900: "#DCDEF0",
              DEFAULT: "#C9CBDB",
            },
            },

            foreground: "#C9CBDB",
            content1: "#222246",
            content2: "#2D2B55",
            content3: "#38356A",
            content4: "#454180",
            primary: {
              DEFAULT: "#A68AE1",
              foreground: "#1e1c3f",
            },
            secondary: {
              DEFAULT: "#e591ff",
              foreground: "#1e1c3f",
            },
            success: {
              DEFAULT: "#55F0D7",
              foreground: "#171530",
            },
            warning: {
              DEFAULT: "#FFDB7F",
              foreground: "#171530",
            },
            danger: {
              DEFAULT: "#FF709D",
              foreground: "#1e1c3f",
            },
            focus: "#7EA7FB",
          },
        },
        dark: {
          colors: {
            background: "#0b0e14",
            primary: {
              DEFAULT: "#7c8bff",
              foreground: "#0b0e14",
            },
            focus: "#7c8bff",
          },
        },
      },
    }),
  ],
};
