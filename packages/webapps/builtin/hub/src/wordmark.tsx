import iconDark from './icon-dark.svg';

type Size = 'sm' | 'md' | 'lg';

const TEXT_SIZE: Record<Size, string> = {
  sm: 'text-3xl',
  md: 'text-5xl',
  lg: 'text-6xl',
};

const ICON_SIZE: Record<Size, string> = {
  sm: 'h-10',
  md: 'h-20',
  lg: 'h-28',
};

export function Wordmark({ size = 'md', tagline = false }: { size?: Size; tagline?: boolean }) {
  return (
    <div className="flex flex-col items-center gap-4">
      <img src={iconDark} alt="" className={`${ICON_SIZE[size]} w-auto`} draggable={false} />
      <WordmarkText className={TEXT_SIZE[size]} />
      {tagline && (
        <div className="font-display text-body tracking-wordmark text-accent">
          <span className="font-medium">the thing.</span>{' '}
          <span className="font-extralight">fully open. all yours.</span>
        </div>
      )}
    </div>
  );
}

export function WordmarkText({ className }: { className: string }) {
  return (
    <div className={`font-display tracking-wordmark leading-none text-off-white ${className}`}>
      <span className="font-medium">bridge</span>
      <span className="font-extralight">thing</span>
    </div>
  );
}
