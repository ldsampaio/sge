import { useState, useEffect, useRef } from "react";
import { IconSearch } from "./icons";

interface SearchBarProps {
  onSearch: (query: string) => void;
  disabled?: boolean;
  /** Seed for the input (search-state restore, clear-search). */
  initialValue?: string;
  /** Element rendered adjacent to the input (scope selector, filters). */
  addon?: React.ReactNode;
}

const DEBOUNCE_MS = 300;

export default function SearchBar({ onSearch, disabled, initialValue, addon }: SearchBarProps) {
  const [value, setValue] = useState(initialValue ?? "");
  const debounceRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const lastEmittedRef = useRef<string | null>(null);

  // Restore/clear seed: applied only when the seed itself changes, never
  // while typing (typing flows child→parent via onSearch, not back down).
  useEffect(() => {
    setValue(initialValue ?? "");
  }, [initialValue]);

  useEffect(() => {
    if (debounceRef.current) {
      clearTimeout(debounceRef.current);
    }
    debounceRef.current = setTimeout(() => {
      if (lastEmittedRef.current !== value) {
        lastEmittedRef.current = value;
        onSearch(value);
      }
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
  }

  return (
    <div className="search-bar-container">
      <IconSearch size={17} />
      <input
        type="search"
        className="search-input"
        placeholder="Buscar em todas as pastas: remetente, assunto…"
        value={value}
        onChange={handleChange}
        disabled={disabled}
        aria-label="Buscar avisos e materiais"
      />
      {value.length > 0 && (
        <button
          type="button"
          className="search-clear"
          onClick={handleClear}
          aria-label="Limpar busca"
          disabled={disabled}
        >
          ×
        </button>
      )}
      {addon}
    </div>
  );
}
