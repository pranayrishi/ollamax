import { countUsers } from "@/lib/db";

// Feature 3: the REAL signup count, computed from the users table. We only
// render it once there's a genuine number — we never show a fabricated or
// zero-padded figure.
export async function SignupCounter() {
  let n = 0;
  try {
    n = await countUsers();
  } catch {
    n = 0;
  }
  if (n <= 0) return null;
  return (
    <p className="mt-6 text-sm text-zinc-500">
      Joined by <span className="font-semibold text-zinc-300">{n.toLocaleString()}</span> developers
    </p>
  );
}
