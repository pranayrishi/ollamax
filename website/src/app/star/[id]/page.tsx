import type { Metadata } from "next";
import { auth } from "@/auth";
import { getStarIntent } from "@/lib/db";
import { Nav } from "@/components/Nav";
import { Footer } from "@/components/Footer";
import { GitHubMark } from "@/components/GitHubMark";
import { StarList } from "./StarList";

export const metadata: Metadata = { title: "Support maintainers" };

export default async function StarPage({
  params,
  searchParams,
}: {
  params: Promise<{ id: string }>;
  searchParams: Promise<{ done?: string; ok?: string; fail?: string; err?: string }>;
}) {
  const { id } = await params;
  const sp = await searchParams;
  const session = await auth();
  const userId = session?.user?.accountId;

  // Results view (after the round-trip). Only render the outcome for the OWNER
  // of a genuinely-consumed intent — so a forged `?done=1&ok=99` URL can't show
  // a fake "Starred N repos" confirmation to anyone but its own crafter.
  if (sp.done) {
    const doneIntent = userId ? await getStarIntent(id) : null;
    const owned = !!doneIntent && doneIntent.user_id === userId && doneIntent.consumed;
    if (!owned) {
      return (
        <Shell>
          <h1 className="page-title text-4xl">Link expired</h1>
          <p className="page-lede text-base">This support link is no longer valid.</p>
        </Shell>
      );
    }
    const ok = Number(sp.ok || 0);
    const fail = Number(sp.fail || 0);
    return (
      <Shell>
        <h1 className="page-title text-4xl">Thanks for supporting maintainers ⭐</h1>
        {sp.err === "scope" ? (
          <p className="mt-3 text-amber-300">
            The starring permission wasn&rsquo;t granted, so nothing was starred.
          </p>
        ) : (
          <p className="page-lede text-base">
            Starred <strong className="text-foreground">{ok}</strong> repo(s)
            {fail > 0 ? ` · ${fail} couldn't be starred` : ""}. You can unstar any of them anytime
            from GitHub.
          </p>
        )}
      </Shell>
    );
  }

  if (!userId) {
    const signin = `/api/auth/signin?callbackUrl=${encodeURIComponent(`/star/${id}`)}`;
    return (
      <Shell>
        <h1 className="page-title text-4xl">Support these maintainers</h1>
        <p className="page-lede text-sm">Sign in to review and star the repos.</p>
        <a
          href={signin}
          className="button-primary mt-7 gap-2"
        >
          <GitHubMark className="h-4 w-4" />
          Sign in with GitHub
        </a>
      </Shell>
    );
  }

  const intent = await getStarIntent(id);
  if (!intent || intent.consumed || new Date(intent.expires_at).getTime() < Date.now()) {
    return (
      <Shell>
        <h1 className="page-title text-4xl">Link expired</h1>
        <p className="page-lede text-base">This support link is no longer valid. Start again from the Hub.</p>
      </Shell>
    );
  }
  if (intent.user_id !== userId) {
    return (
      <Shell>
        <h1 className="page-title text-4xl">Not your request</h1>
        <p className="page-lede text-base">This link was created for a different account.</p>
      </Shell>
    );
  }

  return (
    <Shell>
      <h1 className="page-title text-4xl">Support these maintainers</h1>
      <p className="page-lede text-sm">
        Star the open-source repos behind this package to credit and support their maintainers. This
        is entirely optional and up to you — pick all or just some. Nothing is starred unless you
        choose it here, and you can unstar anytime.
      </p>
      <p className="mt-4 text-xs leading-relaxed text-muted-foreground">
        Starring needs a one-time GitHub permission (<code>public_repo</code>), requested only for
        this action. We never star anything automatically.
      </p>
      <StarList id={id} repos={intent.repos} />
    </Shell>
  );
}

function Shell({ children }: { children: React.ReactNode }) {
  return (
    <>
      <Nav />
      <main id="main" className="page-frame max-w-2xl">
        {children}
      </main>
      <Footer />
    </>
  );
}
