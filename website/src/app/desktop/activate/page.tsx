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
      <main id="main" className="page-frame max-w-md">
        <h1 className="page-title text-4xl">Activate the desktop app</h1>
        <p className="page-lede text-sm">
          Enter the code shown in the app to link it to your GitHub account.
        </p>

        {signedIn ? (
          <ActivateForm />
        ) : (
          <a
            href={signinHref}
            className="button-primary mt-8 gap-2"
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
