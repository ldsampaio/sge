/// <reference types="vite/client" />

// Phase 31-03: global injected by learn.rs for HistoryPanel reset button
interface Window {
  __SGE_RESET_LEARN?: () => void;
}
