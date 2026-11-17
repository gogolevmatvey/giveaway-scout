import type { ReactNode } from "react";

interface SettingCardProps {
  controlId: string;
  title: string;
  description: ReactNode;
  children: ReactNode;
}

/** Keeps the setting's label, explanation and action in one visual row. */
export function SettingCard({ controlId, title, description, children }: SettingCardProps) {
  return (
    <div className="setting-card input-setting">
      <div className="setting-card-copy">
        <label htmlFor={controlId}>{title}</label>
        {description}
      </div>
      <div className="setting-card-control">{children}</div>
    </div>
  );
}
