// Auth.js route handler — exposes /api/auth/* (sign-in, callback, session,
// sign-out, CSRF). GitHub is the only configured provider.
import { handlers } from "@/auth";

export const { GET, POST } = handlers;
