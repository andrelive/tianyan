import { useEffect, useRef, useState } from 'react';
import { apiGet, fetchKnowledgeSuggestions } from '@/lib/api-client';
import type { KnowledgeSearchResult, KnowledgeSearchResponse } from '@/lib/types';
import { Search, Loader2, AlertCircle, ChevronDown, ChevronUp, BookOpen } from 'lucide-react';

function HighlightedText({ text, query }: { text: string; query: string }) {
  if (!query.trim()) return <>{text}</>;
  const escaped = query.replace(/[.*+?^${}()|[\]\\]/g, '$&');
  const parts = text.split(new RegExp(`(${escaped})`, 'gi'));
  return (
    <>
      {parts.map((part, i) =>
        part.toLowerCase() === query.toLowerCase() ? (
          <mark
            key={`hl-${i}-${part.substring(0, 5)}`}
            className="bg-yellow-200 dark:bg-yellow-800 rounded-sm px-0.5"
          >
            {part}
          </mark>
        ) : (
          part
        ),
      )}
    </>
  );
}

/** 知识库搜索（防抖搜索 + 建议 + 结果展开）。 */
export default function KnowledgeSearchTab() {
  const [searchQuery, setSearchQuery] = useState('');
  const [searchResults, setSearchResults] = useState<KnowledgeSearchResult[]>([]);
  const [totalResults, setTotalResults] = useState(0);
  const [isSearching, setIsSearching] = useState(false);
  const [searchError, setSearchError] = useState<string | null>(null);
  const [suggestions, setSuggestions] = useState<string[]>([]);
  const [expandedResultId, setExpandedResultId] = useState<string | null>(null);
  const debounceRef = useRef<ReturnType<typeof setTimeout>>();

  useEffect(() => {
    if (!searchQuery.trim()) {
      setSearchResults([]);
      setTotalResults(0);
      return;
    }

    if (debounceRef.current) {
      clearTimeout(debounceRef.current);
    }

    const query = searchQuery.trim();
    debounceRef.current = setTimeout(async () => {
      setIsSearching(true);
      setSearchError(null);
      try {
        const res = await apiGet<KnowledgeSearchResponse>(
          `/knowledge/search?q=${encodeURIComponent(query)}&limit=10`,
        );
        setSearchResults(res.results);
        setTotalResults(res.total);
      } catch (err) {
        setSearchError(err instanceof Error ? err.message : '搜索失败');
        setSearchResults([]);
      } finally {
        setIsSearching(false);
      }
    }, 300);

    return () => {
      if (debounceRef.current) {
        clearTimeout(debounceRef.current);
      }
    };
  }, [searchQuery]);

  useEffect(() => {
    const query = searchQuery.trim();
    if (!query) {
      setSuggestions([]);
      return;
    }

    let cancelled = false;
    const timer = setTimeout(async () => {
      try {
        const res = await fetchKnowledgeSuggestions(query);
        if (!cancelled) {
          setSuggestions(res.suggestions);
        }
      } catch {
        if (!cancelled) {
          setSuggestions([]);
        }
      }
    }, 300);

    return () => {
      cancelled = true;
      clearTimeout(timer);
    };
  }, [searchQuery]);

  const toggleResultExpand = (id: string) => {
    setExpandedResultId((prev) => (prev === id ? null : id));
  };

  return (
    <div className="space-y-4">
      {/* Search input */}
      <div className="relative">
        <Search
          size={18}
          className="absolute left-3 top-1/2 -translate-y-1/2 text-[var(--color-text-tertiary)]"
        />
        <input
          type="text"
          value={searchQuery}
          onChange={(e) => setSearchQuery(e.target.value)}
          placeholder="搜索知识库..."
          aria-label="搜索知识库"
          className="w-full pl-10 pr-10 py-2.5 rounded-lg border border-[var(--color-border)] bg-[var(--color-bg-primary)] text-sm text-[var(--color-text-primary)] placeholder:text-[var(--color-text-tertiary)] focus:outline-none focus:ring-2 focus:ring-blue-500/30 focus:border-blue-500 transition-colors"
        />
        {isSearching && (
          <Loader2
            size={16}
            className="absolute right-3 top-1/2 -translate-y-1/2 text-[var(--color-text-tertiary)] animate-spin"
            aria-label="正在搜索"
          />
        )}
      </div>

      {/* Suggestion chips */}
      {!isSearching && suggestions.length > 0 && (
        <div className="flex flex-wrap gap-2">
          {suggestions.map((s) => (
            <button
              key={s}
              type="button"
              onClick={() => setSearchQuery(s)}
              className="px-2.5 py-1 text-xs rounded-md border border-[var(--color-border)] bg-[var(--color-bg-secondary)] text-[var(--color-text-secondary)] hover:border-blue-400 hover:text-blue-600 transition-colors"
            >
              {s}
            </button>
          ))}
        </div>
      )}

      {/* Search error */}
      {searchError && (
        <div
          role="alert"
          className="flex items-center gap-2 p-3 rounded-lg bg-red-50 dark:bg-red-950 border border-red-200 dark:border-red-800 text-red-700 dark:text-red-300 text-sm"
        >
          <AlertCircle size={16} />
          <span>{searchError}</span>
        </div>
      )}

      {/* Results list */}
      {!searchError && totalResults > 0 && (
        <div className="space-y-2">
          <p className="text-xs text-[var(--color-text-tertiary)]">共找到 {totalResults} 条结果</p>
          {searchResults.map((result) => (
            <div
              key={result.id}
              className="rounded-lg border border-[var(--color-border)] overflow-hidden transition-shadow hover:shadow-sm"
            >
              <button
                onClick={() => toggleResultExpand(result.id)}
                className="w-full flex items-start justify-between p-4 text-left hover:bg-[var(--color-bg-hover)] transition-colors"
              >
                <div className="flex-1 min-w-0">
                  <h4 className="text-sm font-medium text-[var(--color-text-primary)] truncate">
                    <HighlightedText text={result.metadata?.title ?? ''} query={searchQuery} />
                  </h4>
                  <div className="flex items-center gap-3 mt-1">
                    <span className="text-xs text-blue-600 dark:text-blue-400 font-medium">
                      {Math.round(result.score * 100)}%
                    </span>
                    <span className="text-xs text-[var(--color-text-tertiary)] truncate">
                      {result.source}
                    </span>
                  </div>
                </div>
                {expandedResultId === result.id ? (
                  <ChevronUp
                    size={16}
                    className="mt-1 shrink-0 text-[var(--color-text-tertiary)]"
                  />
                ) : (
                  <ChevronDown
                    size={16}
                    className="mt-1 shrink-0 text-[var(--color-text-tertiary)]"
                  />
                )}
              </button>
              {expandedResultId === result.id && (
                <div className="px-4 pb-4">
                  <div className="p-3 rounded-md bg-[var(--color-bg-secondary)] text-sm text-[var(--color-text-secondary)] leading-relaxed">
                    <HighlightedText text={result.content} query={searchQuery} />
                  </div>
                </div>
              )}
            </div>
          ))}
        </div>
      )}

      {/* Empty state when searching with no results */}
      {!isSearching && !searchError && searchQuery.trim() !== '' && searchResults.length === 0 && (
        <div className="flex flex-col items-center justify-center py-16 text-[var(--color-text-tertiary)]">
          <Search size={40} className="mb-3 opacity-40" />
          <p className="text-sm">未找到匹配结果</p>
        </div>
      )}

      {/* Initial empty state */}
      {searchQuery.trim() === '' && !searchError && (
        <div className="flex flex-col items-center justify-center py-16 text-[var(--color-text-tertiary)]">
          <BookOpen size={40} className="mb-3 opacity-40" />
          <p className="text-sm">输入关键词搜索知识库内容</p>
        </div>
      )}
    </div>
  );
}
