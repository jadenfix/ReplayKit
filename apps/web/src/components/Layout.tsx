import type { ReactNode } from 'react';

interface LayoutProps {
  left: ReactNode;
  center: ReactNode;
  right: ReactNode;
  bottom: ReactNode;
  failureNav: ReactNode;
}

export function Layout({ left, center, right, bottom, failureNav }: LayoutProps) {
  return (
    <div className="layout">
      <header className="layout__header">
        <div className="layout__logo">ReplayKit</div>
        <div className="layout__subtitle">Semantic Replay Debugger</div>
      </header>
      <div className="layout__body">
        <aside className="layout__left">{left}</aside>
        <div className="layout__main">
          <div className="layout__center">
            {failureNav}
            {center}
          </div>
          <div className="layout__right">{right}</div>
        </div>
      </div>
      <div className="layout__bottom">{bottom}</div>
    </div>
  );
}
