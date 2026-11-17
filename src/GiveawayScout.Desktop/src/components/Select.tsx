import { useCallback, useEffect, useId, useLayoutEffect, useRef, useState, type KeyboardEvent } from "react";
import { createPortal } from "react-dom";
import { Check, ChevronDown } from "lucide-react";

export interface SelectOption<T extends string = string> {
  value: T;
  label: string;
  disabled?: boolean;
}

export interface SelectProps<T extends string = string> {
  id?: string;
  label: string;
  value: T;
  options: readonly SelectOption<T>[];
  disabled?: boolean;
  onValueChange: (value: T) => void;
  className?: string;
  describedBy?: string;
}

interface PopupPosition {
  top: number;
  left: number;
  width: number;
  maxHeight: number;
  placement: "above" | "below";
}

let closeOpenSelect: (() => void) | null = null;

/** A select-only combobox: focus stays on its trigger while its options are open. */
export function Select<T extends string>({
  id, label, value, options, disabled = false, onValueChange, className, describedBy,
}: SelectProps<T>) {
  const generatedId = useId();
  const triggerId = id ?? `${generatedId}-trigger`;
  const listboxId = `${generatedId}-listbox`;
  const trigger = useRef<HTMLButtonElement>(null);
  const popup = useRef<HTMLDivElement>(null);
  const search = useRef({ text: "", time: 0 });
  const [isOpen, setOpen] = useState(false);
  const [activeIndex, setActiveIndex] = useState(-1);
  const [hoveredIndex, setHoveredIndex] = useState(-1);
  const [keyboardNavigation, setKeyboardNavigation] = useState(false);
  const [position, setPosition] = useState<PopupPosition | null>(null);
  const selectedIndex = options.findIndex(option => option.value === value);
  const lastEnabledIndex = options.reduce((last, option, index) => option.disabled ? last : index, -1);
  const unavailable = disabled || !options.some(option => !option.disabled);

  const close = useCallback(() => {
    setOpen(false);
    setPosition(null);
    setHoveredIndex(-1);
    setKeyboardNavigation(false);
    search.current = { text: "", time: 0 };
    if (closeOpenSelect === close) closeOpenSelect = null;
  }, []);

  const show = (index = selectedIndex) => {
    if (unavailable) return;
    closeOpenSelect?.();
    closeOpenSelect = close;
    setHoveredIndex(-1);
    setActiveIndex(index >= 0 && !options[index]?.disabled
      ? index : options.findIndex(option => !option.disabled));
    setOpen(true);
  };

  const choose = (index: number) => {
    const option = options[index];
    if (!option || option.disabled || unavailable) return;
    onValueChange(option.value);
    close();
    trigger.current?.focus({ preventScroll: true });
  };

  const updatePosition = useCallback(() => {
    const anchor = trigger.current;
    const panel = popup.current;
    if (!anchor || !panel) return;
    const bounds = anchor.getBoundingClientRect();
    const margin = 8;
    const gap = 6;
    const viewportWidth = document.documentElement.clientWidth;
    const viewportHeight = window.innerHeight;
    if (bounds.bottom <= margin || bounds.top >= viewportHeight - margin ||
      bounds.right <= margin || bounds.left >= viewportWidth - margin) {
      close();
      return;
    }
    // A portalled panel must also disappear when its trigger leaves a scroll pane.
    for (let parent = anchor.parentElement; parent; parent = parent.parentElement) {
      const style = getComputedStyle(parent);
      const clip = parent.getBoundingClientRect();
      if ((/(auto|scroll|hidden|clip)/.test(style.overflowY) &&
          (bounds.bottom <= clip.top || bounds.top >= clip.bottom)) ||
        (/(auto|scroll|hidden|clip)/.test(style.overflowX) &&
          (bounds.right <= clip.left || bounds.left >= clip.right))) {
        close();
        return;
      }
    }
    const width = Math.min(bounds.width, Math.max(0, viewportWidth - margin * 2));
    // Measure wrapped option labels at the real trigger width before choosing a side.
    panel.style.width = `${width}px`;
    const contentHeight = panel.scrollHeight + panel.offsetHeight - panel.clientHeight;
    const desiredHeight = Math.min(256, contentHeight);
    const below = Math.max(0, viewportHeight - bounds.bottom - gap - margin);
    const above = Math.max(0, bounds.top - gap - margin);
    const placement = below >= desiredHeight || below >= above ? "below" : "above";
    const maxHeight = Math.min(256, placement === "below" ? below : above);
    const height = Math.min(contentHeight, maxHeight);
    const next: PopupPosition = {
      top: placement === "below" ? bounds.bottom + gap : bounds.top - gap - height,
      left: Math.max(margin, Math.min(bounds.left, viewportWidth - width - margin)),
      width, maxHeight, placement,
    };
    setPosition(previous => previous && previous.top === next.top && previous.left === next.left &&
      previous.width === next.width && previous.maxHeight === next.maxHeight &&
      previous.placement === next.placement ? previous : next);
  }, [close]);

  useLayoutEffect(() => {
    if (isOpen) updatePosition();
  });

  useLayoutEffect(() => {
    if (!isOpen || activeIndex < 0) return;
    const panel = popup.current;
    const option = panel?.children[activeIndex] as HTMLElement | undefined;
    if (!panel || !option) return;
    const panelBounds = panel.getBoundingClientRect();
    const optionBounds = option.getBoundingClientRect();
    if (optionBounds.top < panelBounds.top + 4) panel.scrollTop -= panelBounds.top + 4 - optionBounds.top;
    else if (optionBounds.bottom > panelBounds.bottom - 4) panel.scrollTop += optionBounds.bottom - panelBounds.bottom + 4;
  }, [isOpen, activeIndex, position?.maxHeight]);

  useEffect(() => {
    if (unavailable) close();
  }, [unavailable, close]);

  useEffect(() => () => {
    if (closeOpenSelect === close) closeOpenSelect = null;
  }, [close]);

  useEffect(() => {
    if (!isOpen) return;
    const outside = (event: Event) => {
      const target = event.target;
      if (target instanceof Node && !trigger.current?.contains(target) && !popup.current?.contains(target)) close();
    };
    const observer = new ResizeObserver(updatePosition);
    if (trigger.current) observer.observe(trigger.current);
    if (popup.current) observer.observe(popup.current);
    document.addEventListener("pointerdown", outside, true);
    document.addEventListener("focusin", outside);
    window.addEventListener("scroll", updatePosition, true);
    window.addEventListener("resize", updatePosition);
    window.addEventListener("blur", close);
    return () => {
      observer.disconnect();
      document.removeEventListener("pointerdown", outside, true);
      document.removeEventListener("focusin", outside);
      window.removeEventListener("scroll", updatePosition, true);
      window.removeEventListener("resize", updatePosition);
      window.removeEventListener("blur", close);
    };
  }, [isOpen, close, updatePosition]);

  const move = (direction: 1 | -1) => {
    const from = activeIndex >= 0 ? activeIndex : direction === 1 ? -1 : 0;
    for (let step = 1; step <= options.length; step++) {
      const index = (from + direction * step + options.length) % options.length;
      if (!options[index].disabled) {
        setActiveIndex(index);
        return;
      }
    }
  };

  const onKeyDown = (event: KeyboardEvent<HTMLButtonElement>) => {
    if (unavailable || event.ctrlKey || event.metaKey) return;
    setHoveredIndex(-1);
    setKeyboardNavigation(true);
    if (event.key === "Escape" && isOpen) {
      event.preventDefault();
      event.stopPropagation();
      close();
      return;
    }
    if (event.key === "Tab") {
      if (isOpen) close();
      return;
    }
    if (event.key === "Enter" || event.key === " ") {
      event.preventDefault();
      if (isOpen) choose(activeIndex);
      else show();
      return;
    }
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault();
      if (isOpen && event.altKey && event.key === "ArrowUp") close();
      else if (isOpen) move(event.key === "ArrowDown" ? 1 : -1);
      else show(selectedIndex >= 0 ? selectedIndex : event.key === "ArrowUp"
        ? lastEnabledIndex : undefined);
      return;
    }
    if (event.key === "Home" || event.key === "End") {
      event.preventDefault();
      const index = event.key === "Home" ? options.findIndex(option => !option.disabled)
        : lastEnabledIndex;
      if (!isOpen) show(index);
      else setActiveIndex(index);
      return;
    }
    if (event.altKey || event.key.length !== 1) return;
    event.preventDefault();
    const character = event.key.toLocaleLowerCase();
    const now = Date.now();
    const previous = now - search.current.time < 700 ? search.current.text : "";
    const query = previous && [...previous].every(letter => letter === character) ? character : previous + character;
    search.current = { text: query, time: now };
    const from = isOpen ? activeIndex : selectedIndex;
    for (let step = query.length === 1 ? 1 : 0; step <= options.length; step++) {
      const index = (Math.max(0, from) + step) % options.length;
      if (!options[index].disabled && options[index].label.toLocaleLowerCase().startsWith(query)) {
        if (isOpen) setActiveIndex(index);
        else show(index);
        return;
      }
    }
  };

  return (
    <div className={`app-select${className ? ` ${className}` : ""}`}>
      <button
        id={triggerId}
        ref={trigger}
        className="app-select-trigger"
        type="button"
        role="combobox"
        aria-label={label}
        aria-describedby={describedBy}
        aria-expanded={isOpen}
        aria-haspopup="listbox"
        aria-controls={isOpen ? listboxId : undefined}
        aria-activedescendant={isOpen && activeIndex >= 0 ? `${listboxId}-option-${activeIndex}` : undefined}
        disabled={unavailable}
        data-value={value}
        onClick={() => { setKeyboardNavigation(false); if (isOpen) close(); else show(); }}
        onKeyDown={onKeyDown}
      >
        <span>{options[selectedIndex]?.label ?? label}</span>
        <ChevronDown size={15} aria-hidden="true" />
      </button>
      {isOpen && createPortal(
        <div
          id={listboxId}
          ref={popup}
          className="app-select-popup app-scrollbar"
          role="listbox"
          aria-label={label}
          data-placement={position?.placement}
          onMouseLeave={() => setHoveredIndex(-1)}
          style={position ? {
            top: position.top, left: position.left, width: position.width, maxHeight: position.maxHeight,
          } : { visibility: "hidden", top: 0, left: 0 }}
        >
          {options.map((option, index) => (
            <button
              id={`${listboxId}-option-${index}`}
              key={option.value}
              type="button"
              role="option"
              className="app-select-option"
              tabIndex={-1}
              aria-selected={option.value === value}
              aria-disabled={option.disabled || undefined}
              disabled={option.disabled}
              data-value={option.value}
              data-active={keyboardNavigation && index === activeIndex}
              data-hovered={index === hoveredIndex}
              onMouseDown={event => event.preventDefault()}
              onMouseMove={() => {
                setKeyboardNavigation(false);
                setHoveredIndex(option.disabled ? -1 : index);
                if (!option.disabled) setActiveIndex(index);
              }}
              onClick={() => choose(index)}
            >
              <span>{option.label}</span>
              <span className="app-select-check" aria-hidden="true">
                {option.value === value && <Check size={14} />}
              </span>
            </button>
          ))}
        </div>, document.body,
      )}
    </div>
  );
}
