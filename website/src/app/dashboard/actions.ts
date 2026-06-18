"use server";

import { auth } from "@/auth";
import { deleteUserUsage, setTelemetryOptOut } from "@/lib/db";
import { revalidatePath } from "next/cache";

// Server actions (built-in CSRF protection). Strictly scoped to the signed-in
// user's own data — there is no way to act on another user's analytics.
export async function deleteMyUsage() {
  const s = await auth();
  const uid = s?.user?.accountId;
  if (!uid) return;
  await deleteUserUsage(uid);
  revalidatePath("/dashboard");
}

export async function setTelemetry(optOut: boolean) {
  const s = await auth();
  const uid = s?.user?.accountId;
  if (!uid) return;
  await setTelemetryOptOut(uid, optOut);
  revalidatePath("/dashboard");
}
