import type { Metadata } from "next";
import "./globals.css";

const siteUrl = process.env.NEXT_PUBLIC_SITE_URL || "http://localhost:3000";

export const metadata: Metadata = {
  metadataBase: new URL(siteUrl),
  title: {
    default: "Ollama-Forge — Local-first AI coding, your models, your machine",
    template: "%s · Ollama-Forge",
  },
  description:
    "A local-first AI coding app built on Ollama. Chat, a tool-using research agent, and parallel multi-model builds — running on your own hardware. Bring your own models. Open source.",
  openGraph: {
    title: "Ollama-Forge",
    description:
      "Local-first AI coding on your own hardware. Your code stays on your machine.",
    type: "website",
    url: siteUrl,
  },
  twitter: { card: "summary_large_image" },
  robots: { index: true, follow: true },
};

export default function RootLayout({ children }: { children: React.ReactNode }) {
  return (
    <html lang="en">
      <body style={{ fontFamily: "var(--font-sans)" }}>
        <a
          href="#main"
          className="sr-only focus:not-sr-only focus:absolute focus:left-4 focus:top-4 focus:z-50 focus:rounded-md focus:bg-ember-500 focus:px-4 focus:py-2 focus:text-ink-950"
        >
          Skip to content
        </a>
        {children}
      </body>
    </html>
  );
}
