/* Ícones SVG inline — sem emoji como ícone estrutural (ui-ux-pro-max).
   Traço consistente 1.8px, tamanho via prop `size`. */

interface IconProps {
  size?: number;
  className?: string;
}

function base(size: number, className: string | undefined, path: React.ReactNode) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth={1.8}
      strokeLinecap="round"
      strokeLinejoin="round"
      className={className}
      aria-hidden="true"
      focusable="false"
    >
      {path}
    </svg>
  );
}

export function IconInbox({ size = 20, className }: IconProps) {
  return base(
    size,
    className,
    <>
      <path d="M22 12h-6l-2 3h-4l-2-3H2" />
      <path d="M5.45 5.11 2 12v6a2 2 0 0 0 2 2h16a2 2 0 0 0 2-2v-6l-3.45-6.89A2 2 0 0 0 16.76 4H7.24a2 2 0 0 0-1.79 1.11z" />
    </>,
  );
}

export function IconHome({ size = 20, className }: IconProps) {
  return base(
    size,
    className,
    <>
      <path d="m3 9 9-7 9 7v11a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z" />
      <path d="M9 22V12h6v10" />
    </>,
  );
}

export function IconBook({ size = 20, className }: IconProps) {
  return base(
    size,
    className,
    <>
      <path d="M4 19.5v-15A2.5 2.5 0 0 1 6.5 2H20v20H6.5a2.5 2.5 0 0 1 0-5H20" />
    </>,
  );
}

export function IconHelp({ size = 20, className }: IconProps) {
  return base(
    size,
    className,
    <>
      <circle cx="12" cy="12" r="10" />
      <path d="M9.09 9a3 3 0 0 1 5.83 1c0 2-3 3-3 3" />
      <path d="M12 17h.01" />
    </>,
  );
}

export function IconSearch({ size = 18, className }: IconProps) {
  return base(
    size,
    className,
    <>
      <circle cx="11" cy="11" r="8" />
      <path d="m21 21-4.3-4.3" />
    </>,
  );
}

export function IconPaperclip({ size = 16, className }: IconProps) {
  return base(
    size,
    className,
    <>
      <path d="m21.44 11.05-9.19 9.19a6 6 0 0 1-8.49-8.49l8.57-8.57A4 4 0 1 1 18 8.84l-8.59 8.57a2 2 0 0 1-2.83-2.83l8.49-8.48" />
    </>,
  );
}

export function IconSync({ size = 18, className }: IconProps) {
  return base(
    size,
    className,
    <>
      <path d="M3 12a9 9 0 0 1 9-9 9.75 9.75 0 0 1 6.74 2.74L21 8" />
      <path d="M21 3v5h-5" />
      <path d="M21 12a9 9 0 0 1-9 9 9.75 9.75 0 0 1-6.74-2.74L3 16" />
      <path d="M8 16H3v5" />
    </>,
  );
}

export function IconCap({ size = 28, className }: IconProps) {
  return base(
    size,
    className,
    <>
      <path d="M22 10 12 5 2 10l10 5 10-5z" />
      <path d="M6 12v5c0 1.7 2.7 3 6 3s6-1.3 6-3v-5" />
      <path d="M22 10v6" />
    </>,
  );
}

export function IconMail({ size = 20, className }: IconProps) {
  return base(
    size,
    className,
    <>
      <rect width="20" height="16" x="2" y="4" rx="2" />
      <path d="m22 7-8.97 5.7a1.94 1.94 0 0 1-2.06 0L2 7" />
    </>,
  );
}

export function IconSparkle({ size = 16, className }: IconProps) {
  return base(
    size,
    className,
    <>
      <path d="M12 3v3m0 12v3M5.6 5.6l2.2 2.2m8.4 8.4 2.2 2.2M3 12h3m12 0h3M5.6 18.4l2.2-2.2m8.4-8.4 2.2-2.2" />
      <circle cx="12" cy="12" r="3.2" />
    </>,
  );
}

export function IconArrowLeft({ size = 18, className }: IconProps) {
  return base(
    size,
    className,
    <>
      <path d="m12 19-7-7 7-7" />
      <path d="M19 12H5" />
    </>,
  );
}

export function IconDownload({ size = 16, className }: IconProps) {
  return base(
    size,
    className,
    <>
      <path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4" />
      <path d="m7 10 5 5 5-5" />
      <path d="M12 15V3" />
    </>,
  );
}
