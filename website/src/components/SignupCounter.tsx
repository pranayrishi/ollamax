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
    <p className="mt-6 text-center text-sm text-muted-foreground">
      Joined by <span className="font-medium text-foreground">{n.toLocaleString()}</span> developers
    </p>
  );
}
