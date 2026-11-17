import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type CSSProperties, type FocusEvent, type KeyboardEvent, type ReactNode, type RefCallback } from "react";
import type { NotificationRecord } from "../api";

const gap = 8;
const overscan = 4;

interface RowProps {
  ref: RefCallback<HTMLElement>;
  style: CSSProperties;
  "aria-posinset": number;
  "aria-setsize": number;
}

interface Props {
  records: NotificationRecord[];
  totalCount: number;
  label: string;
  layoutKey: string;
  hasMore: boolean;
  fetching: boolean;
  loadingMore: boolean;
  error: boolean;
  loadingText: string;
  errorText: string;
  retryText: string;
  onLoadMore: () => Promise<unknown>;
  onRetry: () => Promise<unknown>;
  renderRow: (record: NotificationRecord, props: RowProps) => ReactNode;
}

interface Anchor { id: string; relativeTop: number }
interface Geometry { offsets: number[]; sizes: number[]; indexes: Map<string, number>; total: number }

function firstAtOrAfter(offsets: number[], position: number) {
  let left = 0, right = offsets.length;
  while (left < right) {
    const middle = (left + right) >>> 1;
    if (offsets[middle] < position) left = middle + 1;
    else right = middle;
  }
  return Math.min(left, Math.max(0, offsets.length - 1));
}

/** Variable-height rows keep DOM size bounded while preserving an ID-based viewport anchor. */
export function VirtualHistoryList(props: Props) {
  const { records, totalCount, label, layoutKey, hasMore, fetching, loadingMore, error, loadingText, errorText, retryText, onLoadMore, onRetry, renderRow } = props;
  const [viewRecords, setViewRecords] = useState(records);
  const [measurementVersion, setMeasurementVersion] = useState(0);
  const [viewport, setViewport] = useState({ top: 0, height: 0 });
  const [focusedId, setFocusedId] = useState<string | null>(null);
  const list = useRef<HTMLDivElement>(null);
  const heights = useRef(new Map<string, number>());
  const elements = useRef(new Map<string, HTMLElement>());
  const callbacks = useRef(new Map<string, RefCallback<HTMLElement>>());
  const rowObserver = useRef<ResizeObserver | null>(null);
  const resizeFrame = useRef<number | null>(null);
  const anchor = useRef<Anchor | null>(null);
  const pendingFocus = useRef<{ index: number; last: boolean } | null>(null);
  const focusAction = useRef<string | null>(null);
  const committedRecords = useRef(viewRecords);
  const geometry = useMemo<Geometry>(() => {
    const offsets: number[] = [], sizes: number[] = [], indexes = new Map<string, number>();
    let total = 0;
    viewRecords.forEach((record, index) => {
      offsets.push(total);
      const height = heights.current.get(record.id) ?? (record.readAtUtc ? 68 : 84);
      sizes.push(height); indexes.set(record.id, index); total += height + gap;
    });
    return { offsets, sizes, indexes, total: Math.max(0, total - gap) };
  }, [viewRecords, measurementVersion]);
  const currentGeometry = useRef(geometry);

  const captureAnchor = useCallback(() => {
    const root = list.current, layout = currentGeometry.current;
    if (!root || root.scrollTop <= 4 || !committedRecords.current.length) { anchor.current = null; return; }
    let index = firstAtOrAfter(layout.offsets, root.scrollTop);
    if (layout.offsets[index] > root.scrollTop + root.clientHeight && index > 0) index--;
    anchor.current = { id: committedRecords.current[index].id, relativeTop: layout.offsets[index] - root.scrollTop };
  }, []);

  const scheduleMeasurements = useCallback(() => {
    if (resizeFrame.current !== null) return;
    resizeFrame.current = requestAnimationFrame(() => {
      resizeFrame.current = null;
      setMeasurementVersion(version => version + 1);
    });
  }, []);

  useLayoutEffect(() => {
    const root = list.current;
    if (!root) return;
    rowObserver.current = new ResizeObserver(entries => {
      let changed = false;
      for (const entry of entries) {
        const element = entry.target as HTMLElement;
        const id = element.dataset.notificationId;
        const height = element.getBoundingClientRect().height;
        if (id && Math.abs((heights.current.get(id) ?? 0) - height) > .5) {
          if (!changed) captureAnchor();
          heights.current.set(id, height); changed = true;
        }
      }
      if (changed) scheduleMeasurements();
    });
    for (const element of elements.current.values()) rowObserver.current.observe(element);
    let previousWidth = root.clientWidth;
    const observer = new ResizeObserver(() => {
      if (Math.abs(root.clientWidth - previousWidth) > 1) {
        captureAnchor(); previousWidth = root.clientWidth; heights.current.clear();
        for (const [id, element] of elements.current) heights.current.set(id, element.getBoundingClientRect().height);
        scheduleMeasurements();
      }
      setViewport({ top: root.scrollTop, height: root.clientHeight });
    });
    observer.observe(root);
    setViewport({ top: root.scrollTop, height: root.clientHeight });
    return () => { observer.disconnect(); rowObserver.current?.disconnect(); rowObserver.current = null;
      if (resizeFrame.current !== null) cancelAnimationFrame(resizeFrame.current); resizeFrame.current = null; };
  }, [captureAnchor, scheduleMeasurements]);

  useLayoutEffect(() => {
    captureAnchor(); heights.current.clear();
    for (const [id, element] of elements.current) heights.current.set(id, element.getBoundingClientRect().height);
    scheduleMeasurements();
  }, [layoutKey, captureAnchor, scheduleMeasurements]);

  // A refresh can prepend more than a page. Keep the old viewport until its anchor is loaded again.
  useLayoutEffect(() => {
    if (records === committedRecords.current) return;
    const root = list.current;
    if (!root) return;
    const wanted = anchor.current;
    if (wanted && root.scrollTop > 4) {
      const ids = new Set(records.map(record => record.id));
      if (!ids.has(wanted.id)) {
        const old = currentGeometry.current;
        const index = old.indexes.get(wanted.id) ?? 0;
        const neighbours = committedRecords.current.slice(Math.max(0, index - 1), index + 10);
        const survivor = neighbours.find(record => ids.has(record.id));
        if (survivor) {
          anchor.current = { id: survivor.id, relativeTop: old.offsets[old.indexes.get(survivor.id)!] - root.scrollTop };
        } else if (hasMore) {
          if (!fetching && !error) void onLoadMore();
          return;
        } else anchor.current = null;
      }
    }
    committedRecords.current = records;
    setViewRecords(records);
    const ids = new Set(records.map(record => record.id));
    for (const id of heights.current.keys()) if (!ids.has(id)) { heights.current.delete(id); callbacks.current.delete(id); }
  }, [records, hasMore, fetching, error, onLoadMore]);

  useLayoutEffect(() => {
    currentGeometry.current = geometry;
    const root = list.current;
    if (!root) return;
    const saved = anchor.current;
    const index = saved ? geometry.indexes.get(saved.id) : undefined;
    if (saved && index !== undefined) root.scrollTop = Math.max(0, geometry.offsets[index] - saved.relativeTop);
    setViewport(previous => previous.top === root.scrollTop && previous.height === root.clientHeight ? previous : { top: root.scrollTop, height: root.clientHeight });
    captureAnchor();
  }, [geometry, captureAnchor]);

  useLayoutEffect(() => {
    if (focusedId && focusAction.current === "read" && document.activeElement === document.body) {
      elements.current.get(focusedId)?.querySelector<HTMLButtonElement>(".history-open")?.focus({ preventScroll: true });
    }
    const pending = pendingFocus.current;
    if (!pending) return;
    const record = viewRecords[pending.index];
    if (!record) return;
    const root = list.current;
    if (root) root.scrollTop = geometry.offsets[pending.index];
    setViewport(previous => ({ ...previous, top: root?.scrollTop ?? 0 }));
    const frame = requestAnimationFrame(() => {
      const row = elements.current.get(record.id);
      if (!row) return;
      const buttons = row.querySelectorAll<HTMLButtonElement>("button:not(:disabled)");
      (pending.last ? buttons[buttons.length - 1] : buttons[0])?.focus({ preventScroll: true });
      pendingFocus.current = null;
    });
    return () => cancelAnimationFrame(frame);
  }, [viewRecords, geometry, focusedId, viewport.top]);

  useEffect(() => {
    const outside = (event: Event) => {
      if (event.target instanceof Node && !list.current?.contains(event.target)) {
        setFocusedId(null); focusAction.current = null; pendingFocus.current = null;
      }
    };
    document.addEventListener("pointerdown", outside, true);
    document.addEventListener("focusin", outside);
    return () => { document.removeEventListener("pointerdown", outside, true); document.removeEventListener("focusin", outside); };
  }, []);

  useEffect(() => { if (error) pendingFocus.current = null; }, [error]);

  useEffect(() => {
    const root = list.current;
    if (root && hasMore && !fetching && !error && root.scrollHeight - root.scrollTop - root.clientHeight < Math.max(240, root.clientHeight)) void onLoadMore();
  }, [viewport, geometry.total, hasMore, fetching, error, onLoadMore]);

  const start = Math.max(0, firstAtOrAfter(geometry.offsets, viewport.top) - 1 - overscan);
  const end = Math.min(viewRecords.length, firstAtOrAfter(geometry.offsets, viewport.top + viewport.height) + 1 + overscan);
  const indexes = new Set(Array.from({ length: Math.max(0, end - start) }, (_, index) => start + index));
  const focusIndex = focusedId ? geometry.indexes.get(focusedId) : undefined;
  if (focusIndex !== undefined) indexes.add(focusIndex);
  const visibleIndexes = [...indexes].sort((a, b) => a - b);
  const register = (id: string): RefCallback<HTMLElement> => {
    let callback = callbacks.current.get(id);
    if (!callback) {
      callback = element => {
        const old = elements.current.get(id);
        if (old) rowObserver.current?.unobserve(old);
        if (element) { elements.current.set(id, element); rowObserver.current?.observe(element); }
        else elements.current.delete(id);
      };
      callbacks.current.set(id, callback);
    }
    return callback;
  };
  const onFocus = (event: FocusEvent<HTMLDivElement>) => {
    const target = event.target as HTMLElement;
    const row = target.closest<HTMLElement>("[data-notification-id]");
    if (row?.dataset.notificationId) { setFocusedId(row.dataset.notificationId); focusAction.current = target.classList.contains("mark-read-button") ? "read" : "open"; }
  };
  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    if (event.key !== "Tab" || event.ctrlKey || event.metaKey || event.altKey) return;
    const target = event.target as HTMLElement;
    const row = target.closest<HTMLElement>("[data-notification-id]");
    const index = row?.dataset.notificationId ? geometry.indexes.get(row.dataset.notificationId) : undefined;
    if (index === undefined || !row) return;
    const buttons = [...row.querySelectorAll<HTMLButtonElement>("button:not(:disabled)")];
    const next = event.shiftKey ? index - 1 : index + 1;
    const boundary = event.shiftKey ? buttons[0] === target : buttons[buttons.length - 1] === target;
    if (!boundary || next < 0 || (next >= viewRecords.length && (!hasMore || error))) return;
    if (next < viewRecords.length && indexes.has(next)) return;
    event.preventDefault(); pendingFocus.current = { index: next, last: event.shiftKey };
    if (next >= viewRecords.length) { if (!fetching) void onLoadMore(); }
    else { if (list.current) list.current.scrollTop = geometry.offsets[next]; setViewport(previous => ({ ...previous, top: geometry.offsets[next] })); }
  };

  return <div ref={list} className="history-list virtual-history-list" role="feed" aria-label={label}
    aria-busy={fetching || records !== viewRecords} data-history-first-id={viewRecords[0]?.id} data-history-loaded-count={viewRecords.length}
    tabIndex={-1} onFocusCapture={onFocus} onKeyDownCapture={onKeyDown}
    onScroll={() => { captureAnchor(); if (list.current) setViewport({ top: list.current.scrollTop, height: list.current.clientHeight }); }}>
    <div className="virtual-history-content" style={{ height: geometry.total }}>
      {visibleIndexes.map(index => renderRow(viewRecords[index], {
        ref: register(viewRecords[index].id), style: { position: "absolute", top: geometry.offsets[index], left: 0, width: "100%" },
        "aria-posinset": index + 1, "aria-setsize": Math.max(totalCount, viewRecords.length),
      }))}
    </div>
    {(loadingMore || error) && <div className="history-page-status" role="status">
      <span>{error ? errorText : loadingText}</span>
      {error && <button className="secondary-button history-load-more-retry" type="button" onClick={() => void onRetry()}>{retryText}</button>}
    </div>}
  </div>;
}
