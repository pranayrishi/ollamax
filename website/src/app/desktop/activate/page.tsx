import type { Metadata } from "next";
import { auth } from "@/auth";
import { Nav } from "@/components/Nav";
import { Footer } from "@/components/Footer";
import { GitHubMark } from "@/components/GitHubMark";
import { ActivateForm } from "./ActivateForm";

export const metadata: Metadata = { title: "Activate desktop app" };

export default async function ActivatePage() {
  const session = await auth();
  const signedIn = !!session?.user;

  // Bounce through GitHub and return here. We deliberately do NOT carry a
  // prefilled code through the URL — the user types the code from their own app.
  const signinHref = `/api/auth/signin?callbackUrl=${encodeURIComponent("/desktop/activate")}`;

  return (
    <>
      <Nav />
      <main id="main" className="mx-auto max-w-md px-4 py-20">
        <h1 className="text-2xl font-bold tracking-tight text-zinc-50">Activate the desktop app</h1>
        <p className="mt-2 text-sm text-zinc-400">
          Enter the code shown in the app to link it to your GitHub account.
        </p>

        {signedIn ? (
          <ActivateForm />
        ) : (
          <a
            href={signinHref}
            className="mt-8 flex items-center justify-center gap-2 rounded-xl bg-ember-500 px-5 py-3 font-semibold text-ink-950 hover:bg-ember-400"
          >
            <GitHubMark className="h-4 w-4" />
            Sign in with GitHub to continue
          </a>
        )}
      </main>
      <Footer />
    </>
  );
}
