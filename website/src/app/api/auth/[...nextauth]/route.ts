// Auth.js route handler — exposes /api/auth/* (sign-in, callback, session,
// sign-out, CSRF). Configured providers: GitHub and Google (see src/auth.ts).
import { handlers } from "@/auth";

export const { GET, POST } = handlers;
