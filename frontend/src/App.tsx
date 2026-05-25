import { type FormEvent, useCallback, useEffect, useRef, useState } from "react";
import "./App.css";

interface SearchResult {
  key: string;
  snippet: SearchSnippetSegment[];
  score: number;
  size: number;
  last_modified: string;
}

interface SearchSnippetSegment {
  text: string;
  highlighted: boolean;
  start: number;
  end: number;
}

interface SearchResponse {
  query: string;
  count: number;
  results: SearchResult[];
}

function formatBytes(bytes: number): string {
  if (bytes === 0) return "0 B";
  const k = 1024;
  const sizes = ["B", "KB", "MB", "GB", "TB"];
  const i = Math.floor(Math.log(bytes) / Math.log(k));
  return `${parseFloat((bytes / k ** i).toFixed(2))} ${sizes[i]}`;
}

function getInitialQuery(): string {
  const params = new URLSearchParams(window.location.search);
  return params.get("q") || "";
}

function App() {
  const [query, setQuery] = useState(getInitialQuery);
  const [results, setResults] = useState<SearchResult[] | null>(null);
  const [searching, setSearching] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const currentSearchController = useRef<AbortController | null>(null);

  const doSearch = useCallback((q: string) => {
    currentSearchController.current?.abort();
    const controller = new AbortController();
    currentSearchController.current = controller;

    setSearching(true);
    setError(null);

    fetch(`/api/search?q=${encodeURIComponent(q)}`, { signal: controller.signal })
      .then((res) => {
        if (!res.ok) throw new Error(`HTTP ${res.status}`);
        return res.json() as Promise<SearchResponse>;
      })
      .then((data) => {
        if (currentSearchController.current !== controller) return;
        setResults(data.results);
      })
      .catch((err) => {
        if (err instanceof DOMException && err.name === "AbortError") return;
        if (currentSearchController.current === controller) {
          setError(err instanceof Error ? err.message : String(err));
        }
      })
      .finally(() => {
        if (currentSearchController.current === controller) {
          currentSearchController.current = null;
          setSearching(false);
        }
      });
  }, []);

  useEffect(() => {
    const initial = getInitialQuery();
    if (initial) {
      doSearch(initial);
    }
    return () => {
      currentSearchController.current?.abort();
      currentSearchController.current = null;
    };
  }, [doSearch]);

  function handleSearch(e: FormEvent) {
    e.preventDefault();
    const q = query.trim();
    if (!q) return;

    const url = new URL(window.location.href);
    url.searchParams.set("q", q);
    window.history.pushState(null, "", url.toString());

    doSearch(q);
  }

  function handleClear() {
    currentSearchController.current?.abort();
    currentSearchController.current = null;
    setQuery("");
    setResults(null);
    setSearching(false);
    setError(null);
    const url = new URL(window.location.href);
    url.searchParams.delete("q");
    window.history.pushState(null, "", url.pathname);
  }

  return (
    <div className="app">
      <h1>FTS Everywhere</h1>
      <form className="search-form" onSubmit={handleSearch}>
        <input
          className="search-input"
          type="text"
          placeholder="Search file contents..."
          value={query}
          onChange={(e) => setQuery(e.target.value)}
        />
        <button type="submit" disabled={searching}>
          Search
        </button>
        {results !== null && (
          <button type="button" onClick={handleClear}>
            Clear
          </button>
        )}
      </form>

      {searching && <p>Searching...</p>}
      {error && <p className="error">Error: {error}</p>}

      {results !== null && !searching && !error && (
        <div className="search-results">
          <p className="result-count">
            {results.length} result{results.length !== 1 ? "s" : ""} found
          </p>
          {results.map((result) => (
            <div key={result.key} className="search-result">
              <a
                className="result-key"
                href={`/api/presign?key=${encodeURIComponent(result.key)}`}
                target="_blank"
                rel="noopener noreferrer"
              >
                {result.key}
              </a>
              <div className="result-meta">
                {formatBytes(result.size)} &middot;{" "}
                {new Date(result.last_modified).toLocaleString()}
              </div>
              <div className="result-snippet">
                {result.snippet.map((segment) =>
                  segment.highlighted ? (
                    <b key={`${segment.start}-${segment.end}-highlight`}>{segment.text}</b>
                  ) : (
                    <span key={`${segment.start}-${segment.end}-text`}>{segment.text}</span>
                  ),
                )}
              </div>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

export default App;
