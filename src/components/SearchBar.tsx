import { useState, useEffect, useRef } from "react";
import { IconSearch } from "./icons";

interface SearchBarProps {
  onSearch: (query: string) => void;
  disabled?: boolean;
}

const DEBOUNCE_MS = 300;

export default function SearchBar({ onSearch, disabled }: SearchBarProps) {
  const [value, setValue] = useState("");
  const debounceRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const lastEmittedRef = useRef<string | null>(null);

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
        placeholder="Buscar avisos: remetente, assunto…"
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
    </div>
  );
}
