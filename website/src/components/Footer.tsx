import Link from "next/link";
import { GitHubMark } from "./GitHubMark";

const repo = process.env.NEXT_PUBLIC_GITHUB_REPO || "https://github.com/pranayrishi/ollamax";

export function Footer() {
  return (
    <footer className="border-t border-ink-700/60">
      <div className="mx-auto grid max-w-6xl gap-10 px-4 py-14 sm:grid-cols-2 md:grid-cols-4">
        <div className="sm:col-span-2 md:col-span-2">
          <div className="flex items-center gap-2 font-semibold text-zinc-100">
            <span className="grid h-7 w-7 place-items-center rounded-lg bg-ember-500 text-ink-950">⚒</span>
            Ollama-Forge
          </div>
          <p className="mt-3 max-w-xs text-sm text-zinc-500">
            Local-first AI coding on your own hardware. Your code stays on your machine.
          </p>
        </div>

        <div>
          <h3 className="mb-3 text-xs font-semibold uppercase tracking-widest text-zinc-500">Product</h3>
          <ul className="space-y-2 text-sm text-zinc-400">
            <li><a href="/#features" className="hover:text-zinc-100">Features</a></li>
            <li><a href="/#how" className="hover:text-zinc-100">How it works</a></li>
            <li><a href="/#download" className="hover:text-zinc-100">Download</a></li>
            <li><Link href="/account" className="hover:text-zinc-100">Account</Link></li>
          </ul>
        </div>

        <div>
          <h3 className="mb-3 text-xs font-semibold uppercase tracking-widest text-zinc-500">Resources</h3>
          <ul className="space-y-2 text-sm text-zinc-400">
            <li><Link href="/privacy" className="hover:text-zinc-100">Privacy</Link></li>
            <li><a href={repo} className="hover:text-zinc-100">Source code</a></li>
            <li><a href={`${repo}/issues`} className="hover:text-zinc-100">Report an issue</a></li>
          </ul>
        </div>
      </div>

      <div className="border-t border-ink-700/60">
        <div className="mx-auto flex max-w-6xl flex-col items-center justify-between gap-3 px-4 py-6 text-sm text-zinc-500 sm:flex-row">
          <p>© {new Date().getFullYear()} Ollama-Forge · MIT licensed</p>
          <a href={repo} className="flex items-center gap-2 hover:text-zinc-100">
            <GitHubMark className="h-4 w-4" />
            GitHub
          </a>
        </div>
      </div>
    </footer>
  );
}
