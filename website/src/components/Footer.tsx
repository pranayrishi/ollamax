import Link from "next/link";
import { GitHubMark } from "./GitHubMark";

const repo = process.env.NEXT_PUBLIC_GITHUB_REPO || "https://github.com/pranayrishi/ollamax";

export function Footer() {
  return (
    <footer className="border-t border-border bg-secondary/70">
      <div className="mx-auto grid max-w-7xl gap-10 px-6 py-16 sm:px-8 sm:grid-cols-2 md:grid-cols-4">
        <div className="sm:col-span-2 md:col-span-2">
          <div className="font-display text-3xl tracking-tight text-foreground">Ollamax</div>
          <p className="mt-4 max-w-xs text-sm leading-relaxed text-muted-foreground">
            Local-first AI coding on your own hardware. Your code stays on your machine.
          </p>
        </div>

        <div>
          <h3 className="eyebrow mb-4">Product</h3>
          <ul className="space-y-2 text-sm text-muted-foreground">
            <li><a href="/#studio" className="transition-colors hover:text-foreground">Studio</a></li>
            <li><a href="/#how" className="transition-colors hover:text-foreground">How it works</a></li>
            <li><a href="/#download" className="transition-colors hover:text-foreground">Download</a></li>
            <li><Link href="/account" className="transition-colors hover:text-foreground">Account</Link></li>
          </ul>
        </div>

        <div>
          <h3 className="eyebrow mb-4">Resources</h3>
          <ul className="space-y-2 text-sm text-muted-foreground">
            <li><Link href="/privacy" className="transition-colors hover:text-foreground">Privacy</Link></li>
            <li><a href={repo} className="transition-colors hover:text-foreground">Source code</a></li>
            <li><a href={`${repo}/issues`} className="transition-colors hover:text-foreground">Report an issue</a></li>
          </ul>
        </div>
      </div>

      <div className="border-t border-border">
        <div className="mx-auto flex max-w-7xl flex-col items-center justify-between gap-3 px-6 py-6 text-sm text-muted-foreground sm:px-8 sm:flex-row">
          <p>© {new Date().getFullYear()} Ollamax · MIT licensed</p>
          <a href={repo} className="flex items-center gap-2 transition-colors hover:text-foreground">
            <GitHubMark className="h-4 w-4" />
            GitHub
          </a>
        </div>
      </div>
    </footer>
  );
}
