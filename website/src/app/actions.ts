"use server";

// Server actions for web sign-in/out. Used by <form action={...}> in both
// server and client components. Two providers: GitHub and Google.
import { signIn, signOut } from "@/auth";

export async function signInGitHub() {
  await signIn("github", { redirectTo: "/account" });
}

export async function signInGoogle() {
  await signIn("google", { redirectTo: "/account" });
}

export async function signOutAction() {
  await signOut({ redirectTo: "/" });
}
