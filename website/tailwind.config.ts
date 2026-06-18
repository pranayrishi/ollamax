import type { Config } from "tailwindcss";

// Dark, modern theme. The brand accent is a warm "forge ember" amber/orange,
// distinct from the reference site so we're not cloning its identity.
const config: Config = {
  content: ["./src/**/*.{ts,tsx,mdx}"],
  darkMode: "class",
  theme: {
    extend: {
      colors: {
        ink: {
          950: "#0a0a0f",
          900: "#0e0e16",
          800: "#15151f",
          700: "#1c1c28",
          600: "#26263a",
        },
        ember: {
          400: "#ffae57",
          500: "#ff8c2b",
          600: "#f5731a",
        },
      },
      fontFamily: {
        sans: ["var(--font-sans)", "system-ui", "sans-serif"],
        mono: ["var(--font-mono)", "ui-monospace", "monospace"],
      },
      keyframes: {
        floaty: {
          "0%,100%": { transform: "translateY(0)" },
          "50%": { transform: "translateY(-6px)" },
        },
        fadeup: {
          from: { opacity: "0", transform: "translateY(10px)" },
          to: { opacity: "1", transform: "translateY(0)" },
        },
      },
      animation: {
        floaty: "floaty 6s ease-in-out infinite",
        fadeup: "fadeup 0.5s ease-out both",
      },
    },
  },
  plugins: [],
};

export default config;
