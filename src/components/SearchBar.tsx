import { useState, useEffect, useRef } from "react";

interface SearchBarProps {
  onSearch: (query: string) => void;
  disabled?: boolean;
}

const DEBOUNCE_MS = 300;

/**
 * Search input with 300ms debounce. Calls onSearch with the debounced
 * query value. An empty query restores the full message list.
 *
 * The parent component owns the data flow — this component only emits
 * search terms, it does not invoke Tauri commands directly.
 */
export default function SearchBar({ onSearch, disabled }: SearchBarProps) {
  const [value, setValue] = useState("");
  const debounceRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(() => {
    // Debounce: wait DEBOUNCE_MS after the last keystroke before firing.
    if (debounceRef.current) {
      clearTimeout(debounceRef.current);
    }
    debounceRef.current = setTimeout(() => {
      onSearch(value);
    }, DEBOUNCE_MS);

    return () => {
      if (debounceRef.current) {
        clearTimeout(debounceRef.current);
      }
    };
  }, [value, onSearch]);

  function handleChange(e: React.ChangeEvent<HTMLInputElement>) {
    setValue(e.currentTarget.value);
  }

  function handleClear() {
    setValue("");
    // onSearch("") fires via the debounce effect above.
  }

  return (
    <div className="search-bar-container">
      <input
        type="search"
        className="search-input"
        placeholder="Search sender, subject…"
        value={value}
        onChange={handleChange}
        disabled={disabled}
        aria-label="Search messages"
      />
      {value.length > 0 && (
        <button
          type="button"
          className="search-clear"
          onClick={handleClear}
          aria-label="Clear search"
          disabled={disabled}
        >
          ×
        </button>
      )}
    </div>
  );
}
