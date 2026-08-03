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
