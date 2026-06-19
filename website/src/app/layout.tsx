import type { Metadata } from "next";
import "./globals.css";

const siteUrl = process.env.NEXT_PUBLIC_SITE_URL || "http://localhost:3000";

export const metadata: Metadata = {
  metadataBase: new URL(siteUrl),
  title: {
    default: "Ollamax — Local-first AI coding on your own hardware",
    template: "%s · Ollamax",
  },
  description:
    "Ollamax is a local-first AI coding app. Chat, an autonomous agent that edits files safely, and on-device voice navigation — all running on your machine through local Ollama. No cloud, no API keys, no telemetry of your code. Open source (MIT).",
  openGraph: {
    title: "Ollamax — Local-first AI coding",
    description:
      "Chat, an autonomous coding agent, and on-device voice — running on your own hardware. Your code never leaves your machine.",
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
