import type { Metadata } from "next";
import { Instrument_Serif, Inter } from "next/font/google";
import "./globals.css";

const siteUrl = process.env.NEXT_PUBLIC_SITE_URL || "http://localhost:3000";

const bodyFont = Inter({
  subsets: ["latin"],
  weight: ["400", "500"],
  display: "swap",
  variable: "--font-body",
});

const displayFont = Instrument_Serif({
  subsets: ["latin"],
  weight: "400",
  display: "swap",
  variable: "--font-display",
});

export const metadata: Metadata = {
  metadataBase: new URL(siteUrl),
  title: {
    default: "Ollamax — Local-first AI coding on your own hardware",
    template: "%s · Ollamax",
  },
  description:
    "Ollamax is a local-first AI coding app. Chat, an approval-gated coding agent, local voice commands, and selected screen-region context use local Ollama or explicitly configured loopback runtimes. Open source (MIT).",
  openGraph: {
    title: "Ollamax — Local-first AI coding",
    description:
      "Chat, an approval-gated coding agent, local voice, and selected screen-region context — on your own hardware, without a cloud-inference route.",
    type: "website",
    url: siteUrl,
  },
  twitter: { card: "summary_large_image" },
  robots: { index: true, follow: true },
};

export default function RootLayout({ children }: { children: React.ReactNode }) {
  return (
    <html lang="en">
      <body className={`${bodyFont.variable} ${displayFont.variable}`}>
        <a
          href="#main"
          className="sr-only focus:not-sr-only focus:absolute focus:left-4 focus:top-4 focus:z-50 focus:rounded-full focus:bg-primary focus:px-4 focus:py-2 focus:text-primary-foreground"
        >
          Skip to content
        </a>
        {children}
      </body>
    </html>
  );
}
