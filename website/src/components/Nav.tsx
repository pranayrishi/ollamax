import Link from "next/link";
import { auth } from "@/auth";
import { signInGitHub, signOutAction } from "@/app/actions";
import { GitHubMark } from "./GitHubMark";

// Sticky top nav. Server component: reads the session and shows either
// "Sign in with GitHub" or the signed-in avatar + Sign out.
export async function Nav() {
  const session = await auth();
  const user = session?.user;
  const avatar = user?.image ?? undefined;
  const login = user?.login;

  return (
    <header className="sticky top-0 z-40 border-b border-ink-700/60 bg-ink-950/80 backdrop-blur">
      <nav className="mx-auto flex max-w-6xl items-center justify-between px-4 py-3" aria-label="Main">
        <Link href="/" className="flex items-center gap-2 font-semibold text-zinc-100">
          <span className="grid h-7 w-7 place-items-center rounded-lg bg-ember-500 text-ink-950">⚒</span>
          Ollama-Forge
        </Link>

        <div className="hidden items-center gap-6 text-sm text-zinc-400 md:flex">
          <a href="/#features" className="hover:text-zinc-100">Features</a>
          <a href="/#how" className="hover:text-zinc-100">How it works</a>
          <a href="/#privacy" className="hover:text-zinc-100">Privacy</a>
          <a href="/#faq" className="hover:text-zinc-100">FAQ</a>
        </div>

        <div className="flex items-center gap-3">
          {user ? (
            <>
              <Link
                href="/account"
                className="flex items-center gap-2 rounded-full border border-ink-600 bg-ink-800 py-1 pl-1 pr-3 text-sm text-zinc-200 hover:border-ember-500"
              >
                {/* eslint-disable-next-line @next/next/no-img-element */}
                {avatar ? (
                  <img src={avatar} alt="" className="h-6 w-6 rounded-full" />
                ) : (
                  <span className="grid h-6 w-6 place-items-center rounded-full bg-ink-600">@</span>
                )}
                <span className="max-w-[10ch] truncate">{login || "account"}</span>
              </Link>
              <form action={signOutAction}>
                <button className="text-sm text-zinc-400 hover:text-zinc-100" type="submit">
                  Sign out
                </button>
              </form>
            </>
          ) : (
            <form action={signInGitHub}>
              <button
                type="submit"
                className="flex items-center gap-2 rounded-lg border border-ink-600 bg-ink-800 px-3 py-1.5 text-sm font-medium text-zinc-100 hover:border-ember-500"
              >
                <GitHubMark className="h-4 w-4" />
                Sign in
              </button>
            </form>
          )}
          <a
            href="/#download"
            className="rounded-lg bg-ember-500 px-3 py-1.5 text-sm font-semibold text-ink-950 hover:bg-ember-400"
          >
            Download
          </a>
        </div>
      </nav>
    </header>
  );
}
