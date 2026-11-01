import { cx } from '../lib/cx';
// Copies of brand/svg (npm --prefix app run icon). Bundled by Vite; nothing loads from the network.
import favicon from '../assets/brand/githop-favicon.svg';
import iconLight from '../assets/brand/githop-icon-light.svg';
import lockup from '../assets/brand/githop-lockup-horizontal.svg';
import lockupReverse from '../assets/brand/githop-lockup-horizontal-reverse.svg';
import lockupSmall from '../assets/brand/githop-lockup-horizontal-small.svg';
import lockupReverseSmall from '../assets/brand/githop-lockup-horizontal-reverse-small.svg';
import markColor from '../assets/brand/githop-mark-color.svg';
import markReverse from '../assets/brand/githop-mark-reverse.svg';
import markInk from '../assets/brand/githop-mark-mono-ink.svg';
import markWhite from '../assets/brand/githop-mark-mono-white.svg';

/**
 * GitHop brand assets (brand/README.md) behind one API so every screen picks
 * the right variant:
 *
 * - At 32 px and below the kit's heavier favicon geometry is used.
 * - Sizes are nominal px at 100 % Windows text size and scale with it (rem),
 *   like icons and avatars. width/height attributes reserve the box, so
 *   nothing shifts while the SVG decodes.
 * - Windows contrast themes don't recolour images. The tile carries its own
 *   white surface and border, so it stays visible on any background; the
 *   lockup and bare mark swap to their reverse variants on dark contrast themes.
 */

const NAME = 'GitHop';
const SMALL_MAX = 32;
const DARK_CONTRAST = '(forced-colors: active) and (prefers-color-scheme: dark)';
/** Horizontal lockup viewBox: 196 × 64 (the tile fills the full height). */
const LOCKUP_RATIO = 196 / 64;

const rem = (px: number) => `${px / 14}rem`;

type ImgProps = {
  src: string;
  dark?: string;
  width: number;
  height: number;
  decorative: boolean;
  className?: string;
};

function BrandImg({ src, dark, width, height, decorative, className }: ImgProps) {
  const img = (
    <img
      src={src}
      width={width}
      height={height}
      alt={decorative ? '' : NAME}
      aria-hidden={decorative || undefined}
      draggable={false}
      className={cx('shrink-0 select-none', className)}
      style={{ width: rem(width), height: rem(height) }}
    />
  );
  if (!dark) return img;
  return (
    <picture className="contents">
      <source media={DARK_CONTRAST} srcSet={dark} />
      {img}
    </picture>
  );
}

/**
 * Light app-icon tile — the primary logo. Decorative by default (pass
 * `decorative={false}` when it is the only thing naming the app).
 */
export function BrandTile({
  size,
  decorative = true,
  className,
}: {
  size: number;
  decorative?: boolean;
  className?: string;
}) {
  return (
    <BrandImg
      src={size <= SMALL_MAX ? favicon : iconLight}
      width={size}
      height={size}
      decorative={decorative}
      className={className}
    />
  );
}

/**
 * Mark without a tile, in colour or single-colour ink. The kit has no
 * small-size bare mark, so at 32 px and below this falls back to the tile.
 */
export function BrandMark({
  size,
  tone = 'color',
  decorative = true,
  className,
}: {
  size: number;
  tone?: 'color' | 'mono';
  decorative?: boolean;
  className?: string;
}) {
  if (size <= SMALL_MAX) {
    return <BrandTile size={size} decorative={decorative} className={className} />;
  }
  return (
    <BrandImg
      src={tone === 'mono' ? markInk : markColor}
      dark={tone === 'mono' ? markWhite : markReverse}
      width={size}
      height={size}
      decorative={decorative}
      className={className}
    />
  );
}

/**
 * Horizontal lockup (tile + "GitHop" wordmark; the wordmark's typeface is
 * embedded in the SVG, so it never depends on the UI fonts). `height` is the tile size; keep it a multiple of 16
 * (32, 48, 64) so the width is whole pixels. Minimum 24 (brand/README.md).
 * Names the app by default (alt "GitHop").
 */
export function BrandLockup({
  height,
  decorative = false,
  className,
}: {
  height: number;
  decorative?: boolean;
  className?: string;
}) {
  const small = height <= SMALL_MAX;
  return (
    <BrandImg
      src={small ? lockupSmall : lockup}
      dark={small ? lockupReverseSmall : lockupReverse}
      width={Math.round(height * LOCKUP_RATIO)}
      height={height}
      decorative={decorative}
      className={className}
    />
  );
}
