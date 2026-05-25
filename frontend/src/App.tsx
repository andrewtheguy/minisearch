import { type FormEvent, useCallback, useEffect, useRef, useState } from "react";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import { Input } from "@/components/ui/input";

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
  limit: number;
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
  const [totalCount, setTotalCount] = useState<number | null>(null);
  const [resultLimit, setResultLimit] = useState<number | null>(null);
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
        setTotalCount(data.count);
        setResultLimit(data.limit);
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
    setTotalCount(null);
    setResultLimit(null);
    setSearching(false);
    setError(null);
    const url = new URL(window.location.href);
    url.searchParams.delete("q");
    window.history.pushState(null, "", url.pathname);
  }

  return (
    <div className="mx-auto max-w-4xl px-4 py-8">
      <h1 className="text-3xl font-bold tracking-tight mb-6">FTS Everywhere</h1>

      <form className="flex gap-2 mb-6" onSubmit={handleSearch}>
        <Input
          className="flex-1"
          type="text"
          placeholder="Search file contents..."
          value={query}
          onChange={(e) => setQuery(e.target.value)}
        />
        <Button type="submit" disabled={searching}>
          Search
        </Button>
        {results !== null && (
          <Button type="button" variant="outline" onClick={handleClear}>
            Clear
          </Button>
        )}
      </form>

      {searching && <p className="text-muted-foreground">Searching...</p>}

      {error && (
        <Alert variant="destructive" className="mb-4">
          <AlertDescription>Error: {error}</AlertDescription>
        </Alert>
      )}

      {results !== null && !searching && !error && (
        <div className="space-y-3">
          <p className="text-sm text-muted-foreground">
            {totalCount !== null && resultLimit !== null && totalCount > resultLimit
              ? `Showing first ${results.length} of ${totalCount} results`
              : `${results.length} result${results.length !== 1 ? "s" : ""} found`}
          </p>
          {results.map((result) => (
            <Card key={result.key}>
              <CardContent>
                <a
                  className="text-primary font-semibold hover:underline block mb-1"
                  href={`/api/presign?key=${encodeURIComponent(result.key)}`}
                  target="_blank"
                  rel="noopener noreferrer"
                >
                  {result.key}
                </a>
                <p className="text-sm text-muted-foreground mb-2">
                  {formatBytes(result.size)} &middot;{" "}
                  {new Date(result.last_modified).toLocaleString()}
                </p>
                <div className="text-sm leading-relaxed font-mono">
                  {result.snippet.map((segment) =>
                    segment.highlighted ? (
                      <mark
                        key={`${segment.start}-${segment.end}-highlight`}
                        className="bg-yellow-100 dark:bg-yellow-900/50 px-0.5 rounded-sm"
                      >
                        {segment.text}
                      </mark>
                    ) : (
                      <span key={`${segment.start}-${segment.end}-text`}>{segment.text}</span>
                    ),
                  )}
                </div>
              </CardContent>
            </Card>
          ))}
        </div>
      )}
    </div>
  );
}

export default App;
