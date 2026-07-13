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
      <main id="main" className="page-frame max-w-2xl">
        <h1 className="page-title">Your account</h1>

        {!user ? (
          <div className="surface mt-10 p-8 text-center">
            <p className="text-muted-foreground">Sign in to view your account.</p>
            <form action={signInGitHub} className="mt-5 flex justify-center">
              <button
                type="submit"
                className="button-primary gap-2"
              >
                <GitHubMark className="h-4 w-4" />
                Sign in with GitHub
              </button>
            </form>
          </div>
        ) : (
          <div className="mt-8 space-y-6">
            {sp.linked && (
              <p className="surface-subtle p-4 text-sm text-foreground/85">
                ✓ Linked your {sp.linked} account.
              </p>
            )}
            {sp.error === "conflict" && (
              <p className="rounded-xl border border-red-500/30 bg-red-500/10 p-3 text-sm text-red-300">
                That identity is already linked to a different Ollamax account.
              </p>
            )}

            <div className="surface flex items-center gap-4">
              {/* eslint-disable-next-line @next/next/no-img-element */}
              {user.image ? (
                <img src={user.image} alt="" className="h-16 w-16 rounded-full" />
              ) : (
                <div className="grid h-16 w-16 place-items-center rounded-full bg-muted text-2xl text-muted-foreground">@</div>
              )}
              <div>
                <p className="text-lg font-medium text-foreground">{user.name || user.login || "Account"}</p>
                <p className="text-sm text-muted-foreground">{user.email || "no email shared"}</p>
              </div>
            </div>

            {/* Linked identities (Round 6 multi-identity) */}
            <div className="surface">
              <h2 className="eyebrow mb-5">
                Linked sign-in methods
              </h2>
              <ProviderRow name="GitHub" linked={hasGitHub} linkHref="/api/link/start?provider=github" />
              {/* FUTURE: Google sign-in disabled for now (see src/auth.ts) — hidden from users. */}
              <p className="mt-4 text-xs leading-relaxed text-muted-foreground">
                Link both to use one account everywhere. <strong>GitHub-only features</strong> (e.g.
                supporting maintainers by starring) need a linked GitHub account — you&rsquo;ll be
                prompted to link it the first time you use them.
              </p>
            </div>

            <div className="flex flex-wrap items-center gap-3">
              <Link
                href="/dashboard"
                className="button-primary px-5"
              >
                View usage dashboard →
              </Link>
              <form action={signOutAction}>
                <button
                  type="submit"
                  className="button-secondary px-5 hover:border-red-500/60 hover:text-red-300"
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
    <div className="flex items-center justify-between border-t border-border py-4 first:border-0">
      <span className="text-foreground">{name}</span>
      {linked ? (
        <span className="text-sm text-foreground">✓ Linked</span>
      ) : (
        <a href={linkHref} className="button-secondary min-h-9 px-4 py-1 text-sm">
          Link {name}
        </a>
      )}
    </div>
  );
}
