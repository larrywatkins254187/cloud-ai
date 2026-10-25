import { createContext, useContext } from "react";
import type { User } from "./types";

export interface AuthState {
  user: User;
  version: string;
  refresh: () => void;
  logout: () => void;
}

export const AuthCtx = createContext<AuthState | null>(null);

export function useAuth(): AuthState {
  const a = useContext(AuthCtx);
  if (!a) throw new Error("not logged in");
  return a;
}

export function useIsAdmin() {
  return useAuth().user.role === "admin";
}
