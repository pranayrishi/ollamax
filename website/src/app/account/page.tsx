import type { Metadata } from "next";
import Link from "next/link";
import { auth } from "@/auth";
import { signInGitHub, signOutAction } from "@/app/actions";
import { Nav } from "@/components/Nav";
import { Footer } from "@/components/Footer";
import { GitHubMark } from "@/components/GitHubMark";

export const metadata: Metadata = { title: "Account" };
export const dynamic = "force-dynamic";

export default async function AccountPage({
  searchParams,
}: {
  searchParams: Promise<{ linked?: string; error?: string }>;
}) {
  const sp = await searchParams;
  const session = await auth();
  const user = session?.user;
  const providers = user?.providers ?? [];
  const hasGitHub = providers.includes("github");

  return (
    <>
      <Nav />
      <main id="main" className="mx-auto max-w-2xl px-4 py-16">
        <h1 className="text-3xl font-bold tracking-tight text-zinc-50">Your account</h1>

        {!user ? (
          <div className="mt-8 rounded-2xl border border-ink-700 bg-ink-900/60 p-8 text-center">
            <p className="text-zinc-400">Sign in to view your account.</p>
            <form action={signInGitHub} className="mt-5 flex justify-center">
              <button
                type="submit"
                className="flex items-center gap-2 rounded-xl bg-ember-500 px-5 py-2.5 font-semibold text-ink-950 hover:bg-ember-400"
              >
                <GitHubMark className="h-4 w-4" />
                Sign in with GitHub
              </button>
            </form>
          </div>
        ) : (
          <div className="mt-8 space-y-6">
            {sp.linked && (
              <p className="rounded-xl border border-ember-500/30 bg-ember-500/[0.06] p-3 text-sm text-zinc-200">
                ✓ Linked your {sp.linked} account.
              </p>
            )}
            {sp.error === "conflict" && (
              <p className="rounded-xl border border-red-500/30 bg-red-500/10 p-3 text-sm text-red-300">
                That identity is already linked to a different Ollama-Forge account.
              </p>
            )}

            <div className="flex items-center gap-4 rounded-2xl border border-ink-700 bg-ink-900/60 p-6">
              {/* eslint-disable-next-line @next/next/no-img-element */}
              {user.image ? (
                <img src={user.image} alt="" className="h-16 w-16 rounded-full" />
              ) : (
                <div className="grid h-16 w-16 place-items-center rounded-full bg-ink-700 text-2xl">@</div>
              )}
              <div>
                <p className="text-lg font-semibold text-zinc-100">{user.name || user.login || "Account"}</p>
                <p className="text-sm text-zinc-500">{user.email || "no email shared"}</p>
              </div>
            </div>

            {/* Linked identities (Round 6 multi-identity) */}
            <div className="rounded-2xl border border-ink-700 bg-ink-900/60 p-6">
              <h2 className="mb-4 text-sm font-semibold uppercase tracking-widest text-zinc-500">
                Linked sign-in methods
              </h2>
              <ProviderRow name="GitHub" linked={hasGitHub} linkHref="/api/link/start?provider=github" />
              {/* FUTURE: Google sign-in disabled for now (see src/auth.ts) — hidden from users. */}
              <p className="mt-3 text-xs text-zinc-500">
                Link both to use one account everywhere. <strong>GitHub-only features</strong> (e.g.
                supporting maintainers by starring) need a linked GitHub account — you&rsquo;ll be
                prompted to link it the first time you use them.
              </p>
            </div>

            <div className="flex flex-wrap items-center gap-3">
              <Link
                href="/dashboard"
                className="rounded-xl bg-ember-500 px-4 py-2 text-sm font-semibold text-ink-950 hover:bg-ember-400"
              >
                View usage dashboard →
              </Link>
              <form action={signOutAction}>
                <button
                  type="submit"
                  className="rounded-xl border border-ink-600 bg-ink-800 px-4 py-2 text-sm text-zinc-200 hover:border-red-500/60 hover:text-red-300"
                >
                  Sign out
                </button>
              </form>
            </div>
          </div>
        )}
      </main>
      <Footer />
    </>
  );
}

function ProviderRow({ name, linked, linkHref }: { name: string; linked: boolean; linkHref: string }) {
  return (
    <div className="flex items-center justify-between border-t border-ink-700/60 py-3 first:border-0">
      <span className="text-zinc-200">{name}</span>
      {linked ? (
        <span className="text-sm text-ember-400">✓ Linked</span>
      ) : (
        <a href={linkHref} className="rounded-lg border border-ink-600 bg-ink-800 px-3 py-1 text-sm text-zinc-200 hover:border-ember-500">
          Link {name}
        </a>
      )}
    </div>
  );
}
