import React from "react";
import ReactDOM from "react-dom/client";
import { HeroUIProvider } from "@heroui/react";
import { Provider as JotaiProvider, useAtomValue } from "jotai";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import "@xterm/xterm/css/xterm.css";
import "./index.css";
import App from "./App";
import { themeAtom } from "./state/atoms";

const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      retry: 1,
      refetchOnWindowFocus: false,
    },
  },
});

/**
 * The theme class lives on the root element, so switching is one atom write.
 * Both classes are full HeroUI themes; "night-rider" extends "dark", so
 * anything not restyled falls back to the original look rather than to light.
 */
function ThemedRoot() {
  const theme = useAtomValue(themeAtom);
  return (
    <main className={`${theme} text-foreground bg-background`}>
      <App />
    </main>
  );
}

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <QueryClientProvider client={queryClient}>
      <JotaiProvider>
        {/* disableAnimation: skips ALL framer-motion animations. Tauri's
            WKWebView hangs on animated overlay enter/exit + backdrop-filter,
            which was freezing the app on modal open/close. Instant overlays are
            the reliable choice here. */}
        <HeroUIProvider disableAnimation>
          <ThemedRoot />
        </HeroUIProvider>
      </JotaiProvider>
    </QueryClientProvider>
  </React.StrictMode>,
);
