import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { BrowserRouter } from "react-router";
import App from "./App";
import { ApiError } from "./lib/api";
import { ConfirmProvider, ToastProvider } from "./components/ui";
import "./styles.css";

const qc = new QueryClient({
  defaultOptions: {
    queries: {
      // Retry network and server errors a few times (with backoff); a 4xx won't fix itself.
      retry: (count, err) => !(err instanceof ApiError && err.status < 500) && count < 3,
      retryDelay: (n) => Math.min(8_000, 1_000 * 2 ** n),
      refetchOnWindowFocus: true,
      refetchOnReconnect: true,
      staleTime: 5_000,
    },
  },
});

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <QueryClientProvider client={qc}>
      <ToastProvider>
        <ConfirmProvider>
          <BrowserRouter>
            <App />
          </BrowserRouter>
        </ConfirmProvider>
      </ToastProvider>
    </QueryClientProvider>
  </StrictMode>,
);

if ("serviceWorker" in navigator && location.hostname !== "localhost") {
  navigator.serviceWorker.register("/sw.js").catch(() => {});
}
