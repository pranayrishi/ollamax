import type { Config } from "tailwindcss";

// Components use a small semantic palette so public marketing pages and the
// authenticated account surfaces share one calm, cinematic visual language.
const config: Config = {
  content: ["./src/**/*.{ts,tsx,mdx}"],
  darkMode: "class",
  theme: {
    extend: {
      colors: {
        background: "hsl(var(--background) / <alpha-value>)",
        foreground: "hsl(var(--foreground) / <alpha-value>)",
        muted: "hsl(var(--muted) / <alpha-value>)",
        "muted-foreground": "hsl(var(--muted-foreground) / <alpha-value>)",
        primary: "hsl(var(--primary) / <alpha-value>)",
        "primary-foreground": "hsl(var(--primary-foreground) / <alpha-value>)",
        secondary: "hsl(var(--secondary) / <alpha-value>)",
        accent: "hsl(var(--accent) / <alpha-value>)",
        border: "hsl(var(--border) / <alpha-value>)",
        input: "hsl(var(--input) / <alpha-value>)",
        ring: "hsl(var(--ring) / <alpha-value>)",
        // Compatibility aliases for existing product controls. They resolve to
        // the neutral system rather than preserving the retired amber UI.
        ink: {
          950: "hsl(var(--background) / <alpha-value>)",
          900: "hsl(var(--secondary) / <alpha-value>)",
          800: "hsl(var(--muted) / <alpha-value>)",
          700: "hsl(var(--border) / <alpha-value>)",
          600: "hsl(var(--input) / <alpha-value>)",
        },
        ember: {
          300: "hsl(var(--foreground) / 0.72)",
          400: "hsl(var(--foreground) / 0.84)",
          500: "hsl(var(--primary) / <alpha-value>)",
          600: "hsl(var(--foreground) / 0.72)",
        },
      },
      fontFamily: {
        sans: ["var(--font-body)", "Inter", "system-ui", "sans-serif"],
        display: ["var(--font-display)", "Instrument Serif", "serif"],
        mono: ["var(--font-mono)", "ui-monospace", "monospace"],
      },
      keyframes: {
        "fade-rise": {
          from: { opacity: "0", transform: "translateY(24px)" },
          to: { opacity: "1", transform: "translateY(0)" },
        },
      },
      animation: {
        "fade-rise": "fade-rise 0.8s ease-out both",
      },
    },
  },
  plugins: [],
};

export default config;
