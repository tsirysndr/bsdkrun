import React from "react";
import ReactDOM from "react-dom/client";
import { HeroUIProvider } from "@heroui/react";
import { Provider as JotaiProvider } from "jotai";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import "@xterm/xterm/css/xterm.css";
import "./index.css";
import App from "./App";

const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      retry: 1,
      refetchOnWindowFocus: false,
    },
  },
});

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <QueryClientProvider client={queryClient}>
      <JotaiProvider>
        {/* disableAnimation: skips ALL framer-motion animations. Tauri's
            WKWebView hangs on animated overlay enter/exit + backdrop-filter,
            which was freezing the app on modal open/close. Instant overlays are
            the reliable choice here. */}
        <HeroUIProvider disableAnimation>
          <main className="dark text-foreground bg-background">
            <App />
          </main>
        </HeroUIProvider>
      </JotaiProvider>
    </QueryClientProvider>
  </React.StrictMode>,
);
