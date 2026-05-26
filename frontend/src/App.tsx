import { Check, ClipboardCopy, Download, X } from "lucide-react";
import { type FormEvent, useCallback, useEffect, useRef, useState } from "react";
import { Navigate, Route, Routes, useNavigate, useParams, useSearchParams } from "react-router";
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
  page: number;
  total_pages: number;
  results: SearchResult[];
}

interface BrowseFolder {
  key: string;
  name: string;
}

interface BrowseFile {
  key: string;
  name: string;
  size: number;
  last_modified: string;
}

interface BrowseResponse {
  prefix: string;
  folders: BrowseFolder[];
  files: BrowseFile[];
  is_truncated: boolean;
  next_continuation_token: string | null;
}

type SearchMode = "both" | "filename" | "content";

function encodeS3Path(key: string): string {
  return key.split("/").map(encodeURIComponent).join("/");
}

function formatBytes(bytes: number): string {
  if (bytes === 0) return "0 B";
  const k = 1024;
  const sizes = ["B", "KB", "MB", "GB", "TB"];
  const i = Math.floor(Math.log(bytes) / Math.log(k));
  return `${parseFloat((bytes / k ** i).toFixed(2))} ${sizes[i]}`;
}

const EXT_PRESETS: Record<string, string> = {
  code: "rs,py,go,java,kt,swift,c,h,cpp,hpp,cc,rb",
  web: "html,htm,css,scss,less,js,jsx,ts,tsx,vue,svelte",
  config: "json,yaml,yml,toml,ini,conf,cfg,env",
  docs: "md,txt,rst,markdown",
  data: "csv,tsv,json,jsonl,ndjson,xml,sql",
  shell: "sh,bash,zsh,fish,ps1",
};

function extToSet(ext: string): Set<string> {
  return new Set(
    ext
      .split(",")
      .map((s) => s.trim().toLowerCase())
      .filter(Boolean),
  );
}

function matchPreset(ext: string): string {
  if (!ext.trim()) return "";
  const current = extToSet(ext);
  for (const [key, value] of Object.entries(EXT_PRESETS)) {
    const preset = extToSet(value);
    if (current.size === preset.size && [...current].every((e) => preset.has(e))) return key;
  }
  return "custom";
}

function getPageNumbers(current: number, total: number): (number | "ellipsis")[] {
  if (total <= 7) {
    return Array.from({ length: total }, (_, i) => i + 1);
  }
  const pages: (number | "ellipsis")[] = [1];
  const windowStart = Math.max(2, current - 1);
  const windowEnd = Math.min(total - 1, current + 1);
  if (windowStart > 2) pages.push("ellipsis");
  for (let i = windowStart; i <= windowEnd; i++) pages.push(i);
  if (windowEnd < total - 1) pages.push("ellipsis");
  pages.push(total);
  return pages;
}

function BrowseViewGuard() {
  const { profileName, "*": splatPath } = useParams<{ profileName: string; "*": string }>();
  if (!profileName) return <Navigate to="/" replace />;
  const prefix = splatPath || "";
  return <BrowseView profileName={profileName} prefix={prefix} />;
}

function BrowseView({ profileName, prefix }: { profileName: string; prefix: string }) {
  const navigate = useNavigate();
  const [searchParams, setSearchParams] = useSearchParams();
  const [profileDescription, setProfileDescription] = useState<string>("");
  const [lastIndexed, setLastIndexed] = useState<string>("");

  const [folders, setFolders] = useState<BrowseFolder[]>([]);
  const [files, setFiles] = useState<BrowseFile[]>([]);
  const [browseLoading, setBrowseLoading] = useState(true);
  const [browseError, setBrowseError] = useState<string | null>(null);
  const [isTruncated, setIsTruncated] = useState(false);
  const [browsePageIndex, setBrowsePageIndex] = useState(0);
  const [pageTokens, setPageTokens] = useState<Array<string | null>>([null]);
  const browseControllerRef = useRef<AbortController | null>(null);

  const [query, setQuery] = useState(() => searchParams.get("q") || "");
  const [searchResults, setSearchResults] = useState<SearchResult[] | null>(null);
  const [totalCount, setTotalCount] = useState<number | null>(null);
  const [page, setPage] = useState(() => {
    const p = Number.parseInt(searchParams.get("page") || "1", 10);
    return p >= 1 ? p : 1;
  });
  const [totalPages, setTotalPages] = useState(0);
  const [searching, setSearching] = useState(false);
  const [searchError, setSearchError] = useState<string | null>(null);
  const [mode, setMode] = useState<SearchMode>(() => {
    const m = searchParams.get("mode");
    if (m === "filename" || m === "content") return m;
    return "both";
  });
  const [ext, setExt] = useState(() => searchParams.get("ext") || "");
  const [extPreset, setExtPreset] = useState(() => matchPreset(searchParams.get("ext") || ""));
  const searchControllerRef = useRef<AbortController | null>(null);

  const [previewUrl, setPreviewUrl] = useState<string | null>(null);
  const [presignUrl, setPresignUrl] = useState<string | null>(null);
  const [previewFileName, setPreviewFileName] = useState<string | null>(null);
  const [previewKey, setPreviewKey] = useState<string | null>(null);
  const [copyStatus, setCopyStatus] = useState<"idle" | "copied" | "failed">("idle");

  async function openPreview(key: string) {
    const base = `/api/p/${profileName}/presign?key=${encodeURIComponent(key)}`;
    setPresignUrl(base);
    setPreviewFileName(key.split("/").pop() || key);
    setPreviewKey(key);
    setCopyStatus("idle");
    const resp = await fetch(base);
    const { url } = await resp.json();
    setPreviewUrl(url);
  }

  const closePreview = useCallback(() => {
    setPreviewUrl(null);
    setPresignUrl(null);
    setPreviewFileName(null);
    setPreviewKey(null);
  }, []);

  const fetchBrowsePage = useCallback(
    (token: string | null, pageIdx: number) => {
      browseControllerRef.current?.abort();
      const controller = new AbortController();
      browseControllerRef.current = controller;

      setBrowseLoading(true);
      setBrowseError(null);

      const params = new URLSearchParams();
      if (prefix) params.set("prefix", prefix);
      if (token) params.set("continuation_token", token);

      fetch(`/api/p/${profileName}/browse?${params}`, { signal: controller.signal })
        .then((res) => {
          if (!res.ok) throw new Error(`HTTP ${res.status}`);
          return res.json() as Promise<BrowseResponse>;
        })
        .then((data) => {
          if (browseControllerRef.current !== controller) return;
          setFolders(data.folders);
          setFiles(data.files);
          setIsTruncated(data.is_truncated);
          setBrowsePageIndex(pageIdx);
          if (data.next_continuation_token) {
            setPageTokens((prev) => {
              const next = [...prev];
              next[pageIdx + 1] = data.next_continuation_token;
              return next;
            });
          }
        })
        .catch((err) => {
          if (err instanceof DOMException && err.name === "AbortError") return;
          if (browseControllerRef.current === controller) {
            setBrowseError(err instanceof Error ? err.message : String(err));
          }
        })
        .finally(() => {
          if (browseControllerRef.current === controller) setBrowseLoading(false);
        });
    },
    [profileName, prefix],
  );

  useEffect(() => {
    setBrowsePageIndex(0);
    setPageTokens([null]);
    fetchBrowsePage(null, 0);
    closePreview();
    return () => {
      browseControllerRef.current?.abort();
    };
  }, [fetchBrowsePage, closePreview]);

  useEffect(() => {
    const controller = new AbortController();
    fetch(`/api/p/${profileName}/info`, { signal: controller.signal })
      .then((res) =>
        res.ok
          ? (res.json() as Promise<{
              name: string;
              description: string;
              last_indexed: string;
            }>)
          : null,
      )
      .then((data) => {
        if (data) {
          setProfileDescription(data.description);
          setLastIndexed(data.last_indexed);
        }
      })
      .catch((err) => {
        if (err instanceof DOMException && err.name === "AbortError") return;
      });
    return () => controller.abort();
  }, [profileName]);

  const doSearch = useCallback(
    (q: string, p: number, m: SearchMode, e: string) => {
      searchControllerRef.current?.abort();
      const controller = new AbortController();
      searchControllerRef.current = controller;

      setSearching(true);
      setSearchError(null);

      const params = new URLSearchParams({ q });
      if (p > 1) params.set("page", String(p));
      if (m !== "both") params.set("mode", m);
      if (e.trim()) params.set("ext", e.trim());
      if (prefix) params.set("prefix", prefix);

      fetch(`/api/p/${profileName}/search?${params}`, { signal: controller.signal })
        .then((res) => {
          if (!res.ok) throw new Error(`HTTP ${res.status}`);
          return res.json() as Promise<SearchResponse>;
        })
        .then((data) => {
          if (searchControllerRef.current !== controller) return;
          setSearchResults(data.results);
          setTotalCount(data.count);
          setTotalPages(data.total_pages);
        })
        .catch((err) => {
          if (err instanceof DOMException && err.name === "AbortError") return;
          if (searchControllerRef.current === controller) {
            setSearchError(err instanceof Error ? err.message : String(err));
          }
        })
        .finally(() => {
          if (searchControllerRef.current === controller) {
            searchControllerRef.current = null;
            setSearching(false);
          }
        });
    },
    [profileName, prefix],
  );

  const doSearchRef = useRef(doSearch);
  doSearchRef.current = doSearch;

  useEffect(() => {
    const params = new URLSearchParams(window.location.search);
    const q = params.get("q") || "";
    if (q) {
      const p = Number.parseInt(params.get("page") || "1", 10);
      const m = params.get("mode");
      const initialMode: SearchMode = m === "filename" || m === "content" ? m : "both";
      const e = params.get("ext") || "";
      doSearchRef.current(q, p >= 1 ? p : 1, initialMode, e);
    }
    return () => {
      searchControllerRef.current?.abort();
      searchControllerRef.current = null;
    };
  }, []);

  function searchAndUpdateUrl(q: string, p: number, m: SearchMode, e: string) {
    const params = new URLSearchParams();
    if (q.trim()) params.set("q", q.trim());
    if (p > 1) params.set("page", String(p));
    if (m !== "both") params.set("mode", m);
    if (e.trim()) params.set("ext", e.trim());
    setSearchParams(params);
    doSearch(q, p, m, e);
  }

  function handleSearch(e: FormEvent) {
    e.preventDefault();
    const q = query.trim();
    if (!q) return;
    setPage(1);
    searchAndUpdateUrl(q, 1, mode, ext);
  }

  function handlePageChange(newPage: number) {
    setPage(newPage);
    window.scrollTo({ top: 0, behavior: "smooth" });
    searchAndUpdateUrl(query.trim(), newPage, mode, ext);
  }

  function handleClearSearch() {
    searchControllerRef.current?.abort();
    searchControllerRef.current = null;
    setQuery("");
    setSearchResults(null);
    setTotalCount(null);
    setPage(1);
    setTotalPages(0);
    setSearching(false);
    setSearchError(null);
    setMode("both");
    setExt("");
    setExtPreset("");
    setSearchParams(new URLSearchParams());
    setBrowsePageIndex(0);
    setPageTokens([null]);
    fetchBrowsePage(null, 0);
    closePreview();
  }

  useEffect(() => {
    if (!previewUrl) return;
    const handler = (e: KeyboardEvent) => {
      if (e.key === "Escape") closePreview();
    };
    document.addEventListener("keydown", handler);
    document.body.style.overflow = "hidden";
    return () => {
      document.removeEventListener("keydown", handler);
      document.body.style.overflow = "";
    };
  }, [previewUrl, closePreview]);

  const segments = prefix ? prefix.replace(/\/$/, "").split("/") : [];
  const isSearchActive = searchResults !== null || searching || searchError !== null;

  return (
    <div className="px-8 py-8">
      <div className="mb-6">
        <div className="flex items-baseline gap-3">
          <h1 className="text-3xl font-bold tracking-tight">{profileName}</h1>
          {profileDescription && (
            <span className="text-muted-foreground">{profileDescription}</span>
          )}
        </div>
        {lastIndexed && (
          <p className="text-sm text-muted-foreground mt-1">Last indexed: {lastIndexed}</p>
        )}
      </div>

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
        {isSearchActive && (
          <Button type="button" variant="outline" onClick={handleClearSearch}>
            Clear
          </Button>
        )}
      </form>

      <div className="flex flex-wrap gap-4 mb-6 items-center">
        <div className="flex items-center gap-2">
          <label htmlFor="ext-preset" className="text-sm text-muted-foreground whitespace-nowrap">
            Extensions:
          </label>
          <select
            id="ext-preset"
            className="h-9 rounded-md border border-input bg-background px-3 text-sm"
            value={extPreset}
            onChange={(e) => {
              const preset = e.target.value;
              setExtPreset(preset);
              if (preset === "custom") {
                setExt("");
              } else {
                setExt(EXT_PRESETS[preset] || "");
              }
            }}
          >
            <option value="">All types</option>
            <option value="code">Code (rs,py,go,java,...)</option>
            <option value="web">Web (html,css,js,ts,...)</option>
            <option value="config">Config (json,yaml,toml,...)</option>
            <option value="docs">Docs (md,txt,rst)</option>
            <option value="data">Data (csv,json,xml,sql,...)</option>
            <option value="shell">Shell (sh,bash,zsh,...)</option>
            <option value="custom">Custom...</option>
          </select>
          {extPreset === "custom" && (
            <Input
              className="w-48"
              type="text"
              placeholder="e.g. rs,py,js"
              value={ext}
              onChange={(e) => setExt(e.target.value)}
            />
          )}
        </div>
        <fieldset className="flex items-center gap-2 border-none p-0 m-0">
          <legend className="text-sm text-muted-foreground whitespace-nowrap float-left mr-2 p-0">
            Search in:
          </legend>
          <div className="flex gap-1">
            {(["both", "filename", "content"] as const).map((m) => (
              <Button
                key={m}
                type="button"
                variant={mode === m ? "default" : "outline"}
                size="sm"
                onClick={() => setMode(m)}
              >
                {m === "both" ? "All" : m === "filename" ? "Filename" : "Content"}
              </Button>
            ))}
          </div>
        </fieldset>
      </div>

      {isSearchActive ? (
        <div className="space-y-3">
          {searching && <p className="text-muted-foreground">Searching...</p>}

          {searchError && (
            <Alert variant="destructive" className="mb-4">
              <AlertDescription>Error: {searchError}</AlertDescription>
            </Alert>
          )}

          {!searching && !searchError && searchResults && (
            <>
              <p className="text-sm text-muted-foreground">
                {totalCount !== null && totalPages > 1
                  ? `Page ${page} of ${totalPages} (${totalCount} results)`
                  : `${searchResults.length} result${searchResults.length !== 1 ? "s" : ""} found`}
              </p>
              {searchResults.map((result) => (
                <Card
                  key={result.key}
                  className={previewKey === result.key ? "ring-2 ring-primary" : ""}
                >
                  <CardContent>
                    <button
                      type="button"
                      className="text-primary font-semibold hover:underline block mb-1 bg-transparent border-none p-0 cursor-pointer text-left"
                      onClick={() => openPreview(result.key)}
                    >
                      {result.key}
                    </button>
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
              {totalPages > 1 && (
                <nav className="flex items-center justify-center gap-1 pt-4">
                  <Button
                    variant="outline"
                    size="sm"
                    disabled={page <= 1}
                    onClick={() => handlePageChange(1)}
                  >
                    First
                  </Button>
                  <Button
                    variant="outline"
                    size="sm"
                    disabled={page <= 1}
                    onClick={() => handlePageChange(page - 1)}
                  >
                    Previous
                  </Button>
                  {getPageNumbers(page, totalPages).map((p, i) =>
                    p === "ellipsis" ? (
                      <span
                        key={`ellipsis-${i === 1 ? "start" : "end"}`}
                        className="px-1 text-sm text-muted-foreground"
                      >
                        ...
                      </span>
                    ) : (
                      <Button
                        key={p}
                        variant={p === page ? "default" : "outline"}
                        size="sm"
                        onClick={() => handlePageChange(p)}
                      >
                        {p}
                      </Button>
                    ),
                  )}
                  <Button
                    variant="outline"
                    size="sm"
                    disabled={page >= totalPages}
                    onClick={() => handlePageChange(page + 1)}
                  >
                    Next
                  </Button>
                  <Button
                    variant="outline"
                    size="sm"
                    disabled={page >= totalPages}
                    onClick={() => handlePageChange(totalPages)}
                  >
                    Last
                  </Button>
                </nav>
              )}
            </>
          )}
        </div>
      ) : (
        <>
          <nav className="flex items-center gap-1 text-sm mb-4 flex-wrap">
            <button
              type="button"
              className={`hover:underline ${prefix ? "text-primary" : "font-semibold text-foreground"}`}
              onClick={() => prefix && navigate(`/p/${profileName}/browse/`)}
              disabled={!prefix}
            >
              Root
            </button>
            {segments.map((seg, i) => {
              const segPrefix = `${segments.slice(0, i + 1).join("/")}/`;
              const isLast = i === segments.length - 1;
              return (
                <span key={segPrefix} className="flex items-center gap-1">
                  <span className="text-muted-foreground">/</span>
                  <button
                    type="button"
                    className={`hover:underline ${isLast ? "font-semibold text-foreground" : "text-primary"}`}
                    onClick={() =>
                      !isLast && navigate(`/p/${profileName}/browse/${encodeS3Path(segPrefix)}`)
                    }
                    disabled={isLast}
                  >
                    {seg}
                  </button>
                </span>
              );
            })}
          </nav>

          {browseLoading && <p className="text-muted-foreground">Loading...</p>}

          {browseError && (
            <Alert variant="destructive" className="mb-4">
              <AlertDescription>Error: {browseError}</AlertDescription>
            </Alert>
          )}

          {!browseLoading && !browseError && (
            <>
              {folders.length === 0 && files.length === 0 && (
                <p className="text-muted-foreground">This folder is empty.</p>
              )}

              {folders.length > 0 && (
                <div className="space-y-1 mb-4">
                  {folders.map((folder) => (
                    <button
                      key={folder.key}
                      type="button"
                      className="w-full text-left px-3 py-2 rounded-md hover:bg-accent transition-colors flex items-center gap-2"
                      onClick={() =>
                        navigate(`/p/${profileName}/browse/${encodeS3Path(folder.key)}`)
                      }
                    >
                      <span className="text-muted-foreground">&#128193;</span>
                      <span className="font-medium">{folder.name}</span>
                    </button>
                  ))}
                </div>
              )}

              {files.length > 0 && (
                <div className="space-y-1">
                  {files.map((file) => (
                    <button
                      key={file.key}
                      type="button"
                      className={`w-full text-left px-3 py-2 rounded-md hover:bg-accent transition-colors flex items-center gap-2 ${previewKey === file.key ? "bg-accent" : ""}`}
                      onClick={() => openPreview(file.key)}
                    >
                      <span className="text-muted-foreground">&#128196;</span>
                      <span className="flex-1 font-medium">{file.name}</span>
                      <span className="text-sm text-muted-foreground">
                        {formatBytes(file.size)}
                      </span>
                      <span className="text-sm text-muted-foreground">
                        {file.last_modified ? new Date(file.last_modified).toLocaleString() : ""}
                      </span>
                    </button>
                  ))}
                </div>
              )}

              {(browsePageIndex > 0 || isTruncated) && (
                <nav className="flex items-center justify-center gap-2 pt-4">
                  <Button
                    variant="outline"
                    size="sm"
                    disabled={browsePageIndex === 0 || browseLoading}
                    onClick={() =>
                      fetchBrowsePage(pageTokens[browsePageIndex - 1] ?? null, browsePageIndex - 1)
                    }
                  >
                    Previous
                  </Button>
                  <span className="text-sm text-muted-foreground">Page {browsePageIndex + 1}</span>
                  <Button
                    variant="outline"
                    size="sm"
                    disabled={!isTruncated || browseLoading}
                    onClick={() =>
                      fetchBrowsePage(pageTokens[browsePageIndex + 1] ?? null, browsePageIndex + 1)
                    }
                  >
                    Next
                  </Button>
                </nav>
              )}
            </>
          )}
        </>
      )}

      {presignUrl && (
        <div
          role="dialog"
          className="fixed inset-0 z-50 flex flex-col px-8 bg-background/80 backdrop-blur-sm"
          onClick={(e) => {
            if (e.target === e.currentTarget) closePreview();
          }}
          onKeyDown={() => {}}
        >
          <div className="flex flex-col w-full h-[90vh] mt-[5vh] mx-auto rounded-lg border border-border bg-background shadow-lg overflow-hidden">
            <div className="flex items-center justify-between border-b border-border px-4 py-2 shrink-0 gap-2">
              <span className="text-sm font-medium truncate flex-1">{previewFileName}</span>
              <div className="flex items-center gap-1">
                <button
                  type="button"
                  className="inline-flex items-center justify-center rounded-md text-sm font-medium h-8 w-8 hover:bg-accent transition-colors"
                  onClick={() => {
                    const fullUrl = `${window.location.origin}${presignUrl}&download=true`;
                    navigator.clipboard.writeText(fullUrl).then(
                      () => {
                        setCopyStatus("copied");
                        setTimeout(() => setCopyStatus("idle"), 2000);
                      },
                      () => {
                        setCopyStatus("failed");
                        setTimeout(() => setCopyStatus("idle"), 2000);
                      },
                    );
                  }}
                  title={
                    copyStatus === "copied"
                      ? "Copied!"
                      : copyStatus === "failed"
                        ? "Failed to copy"
                        : "Copy URL"
                  }
                >
                  {copyStatus === "copied" ? (
                    <Check className="h-4 w-4 text-green-500" />
                  ) : (
                    <ClipboardCopy className="h-4 w-4" />
                  )}
                </button>
                <a
                  href={`${presignUrl}&download=true`}
                  className="inline-flex items-center justify-center rounded-md text-sm font-medium h-8 w-8 hover:bg-accent transition-colors"
                  title="Download"
                >
                  <Download className="h-4 w-4" />
                </a>
                <button
                  type="button"
                  className="inline-flex items-center justify-center rounded-md text-sm font-medium h-8 w-8 hover:bg-accent transition-colors"
                  onClick={closePreview}
                  title="Close"
                >
                  <X className="h-4 w-4" />
                </button>
              </div>
            </div>
            {previewUrl ? (
              <iframe
                className="flex-1 w-full border-0"
                src={previewUrl}
                sandbox="allow-same-origin"
                referrerPolicy="no-referrer"
                title={`Preview: ${previewFileName}`}
              />
            ) : (
              <div className="flex-1 flex items-center justify-center text-muted-foreground">
                Loading…
              </div>
            )}
          </div>
        </div>
      )}
    </div>
  );
}

function RootRedirect() {
  const navigate = useNavigate();
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const doFetch = useCallback(() => {
    setLoading(true);
    setError(null);
    fetch("/api/default-profile")
      .then((res) => {
        if (!res.ok) throw new Error(`HTTP ${res.status}`);
        return res.json() as Promise<{ name: string }>;
      })
      .then((data) => {
        navigate(`/p/${data.name}/browse/`, { replace: true });
      })
      .catch((err) => {
        setError(err instanceof Error ? err.message : String(err));
        setLoading(false);
      });
  }, [navigate]);

  useEffect(() => {
    doFetch();
  }, [doFetch]);

  if (error) {
    return (
      <div className="flex flex-col items-center justify-center min-h-screen gap-4">
        <p className="text-red-600">Failed to load profile: {error}</p>
        <button
          type="button"
          className="px-4 py-2 text-sm bg-gray-100 hover:bg-gray-200 rounded"
          onClick={doFetch}
        >
          Retry
        </button>
      </div>
    );
  }

  if (loading) {
    return (
      <div className="flex items-center justify-center min-h-screen">
        <p className="text-gray-500">Loading...</p>
      </div>
    );
  }

  return null;
}

function App() {
  return (
    <Routes>
      <Route path="/p/:profileName" element={<Navigate to="browse/" replace />} />
      <Route path="/p/:profileName/browse/*" element={<BrowseViewGuard />} />
      <Route path="*" element={<RootRedirect />} />
    </Routes>
  );
}

export default App;
