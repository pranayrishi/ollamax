import Link from "next/link";
import { auth } from "@/auth";
import { signInGitHub, signOutAction } from "@/app/actions";
import { GitHubMark } from "./GitHubMark";
import { Button } from "./ui/button";

const links = [
  { href: "/", label: "Home", active: true },
  { href: "/#studio", label: "Studio" },
  { href: "/#features", label: "Features" },
  { href: "/#privacy", label: "Privacy" },
  { href: "/#faq", label: "FAQ" },
];

// Session-aware server component shared by the public and signed-in routes.
// The visual shell stays glass-like without moving identity state client-side.
export async function Nav() {
  const session = await auth();
  const user = session?.user;
  const avatar = user?.image ?? undefined;
  const login = user?.login;

  return (
    <header className="relative z-10 px-4 pt-4 sm:px-6 sm:pt-6">
      <nav
        className="liquid-glass mx-auto flex max-w-7xl items-center justify-between rounded-[2rem] px-5 py-4 sm:px-8 sm:py-6"
        aria-label="Main"
      >
        <Link href="/" className="font-display text-3xl tracking-tight text-foreground">Ollamax</Link>

        <div className="hidden items-center gap-6 text-sm md:flex">
          {links.map((link) => (
            <a
              key={link.href}
              href={link.href}
              className={link.active ? "text-foreground" : "text-muted-foreground transition-colors hover:text-foreground"}
            >
              {link.label}
            </a>
          ))}
        </div>

        <div className="flex items-center gap-2 sm:gap-3">
          {user ? (
            <>
              <Link
                href="/account"
                className="liquid-glass flex h-10 max-w-[11rem] items-center gap-2 rounded-full py-1 pl-1 pr-3 text-sm text-foreground transition-transform hover:scale-[1.03]"
              >
                {/* eslint-disable-next-line @next/next/no-img-element */}
                {avatar ? (
                  <img src={avatar} alt="" className="h-8 w-8 rounded-full" />
                ) : (
                  <span className="grid h-8 w-8 place-items-center rounded-full bg-muted text-muted-foreground">@</span>
                )}
                <span className="truncate">{login || "account"}</span>
              </Link>
              <form action={signOutAction} className="hidden sm:block">
                <button className="text-sm text-muted-foreground transition-colors hover:text-foreground" type="submit">
                  Sign out
                </button>
              </form>
            </>
          ) : (
            <form action={signInGitHub} className="hidden sm:block">
              <Button type="submit" variant="glass" size="sm" className="gap-2">
                <GitHubMark className="h-4 w-4" />
                Sign in
              </Button>
            </form>
          )}
          <Button asChild variant="glass" size="sm" className="px-4 sm:px-6">
            <a href="/#download">Begin Journey</a>
          </Button>
        </div>
      </nav>
    </header>
  );
}
