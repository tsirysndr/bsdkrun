import ContentLoader from "react-content-loader";

const BG = "#1b2130";
const FG = "#2b3446";

/** One shimmering table row (scales to the container width via viewBox). */
function RowSkeleton() {
  return (
    <ContentLoader
      speed={1.5}
      width="100%"
      height={52}
      viewBox="0 0 1000 52"
      preserveAspectRatio="none"
      backgroundColor={BG}
      foregroundColor={FG}
      className="border-b border-white/5"
    >
      <circle cx="12" cy="26" r="5" />
      <rect x="30" y="14" rx="4" ry="4" width="200" height="10" />
      <rect x="30" y="30" rx="4" ry="4" width="130" height="8" />
      <rect x="430" y="21" rx="4" ry="4" width="72" height="12" />
      <rect x="560" y="21" rx="4" ry="4" width="90" height="10" />
      <rect x="710" y="21" rx="4" ry="4" width="120" height="10" />
      <rect x="880" y="21" rx="4" ry="4" width="90" height="10" />
    </ContentLoader>
  );
}

/** A placeholder table shown while a list query loads for the first time. */
export function TableSkeleton({ rows = 6 }: { rows?: number }) {
  return (
    <div aria-busy="true" aria-label="Loading">
      {/* Header shimmer */}
      <ContentLoader
        speed={1.5}
        width="100%"
        height={34}
        viewBox="0 0 1000 34"
        preserveAspectRatio="none"
        backgroundColor={BG}
        foregroundColor={FG}
        className="border-b border-white/10"
      >
        <rect x="30" y="14" rx="3" ry="3" width="70" height="8" />
        <rect x="430" y="14" rx="3" ry="3" width="50" height="8" />
        <rect x="560" y="14" rx="3" ry="3" width="50" height="8" />
        <rect x="710" y="14" rx="3" ry="3" width="70" height="8" />
        <rect x="880" y="14" rx="3" ry="3" width="60" height="8" />
      </ContentLoader>
      {Array.from({ length: rows }).map((_, i) => (
        <RowSkeleton key={i} />
      ))}
    </div>
  );
}

/** One shimmering flavor card. */
function CardSkeleton() {
  return (
    <ContentLoader
      speed={1.5}
      width="100%"
      height={132}
      viewBox="0 0 300 132"
      preserveAspectRatio="none"
      backgroundColor={BG}
      foregroundColor={FG}
      className="rounded-xl border border-white/10"
    >
      <rect x="16" y="16" rx="10" ry="10" width="44" height="44" />
      <rect x="72" y="20" rx="4" ry="4" width="90" height="12" />
      <rect x="72" y="40" rx="4" ry="4" width="180" height="8" />
      <rect x="72" y="54" rx="4" ry="4" width="150" height="8" />
      <rect x="16" y="84" rx="4" ry="4" width="70" height="16" />
      <rect x="94" y="84" rx="4" ry="4" width="46" height="16" />
      <rect x="212" y="100" rx="6" ry="6" width="72" height="24" />
    </ContentLoader>
  );
}

/** A placeholder card grid shown while the flavors query loads. */
export function CardGridSkeleton({ cards = 8 }: { cards?: number }) {
  return (
    <div
      aria-busy="true"
      aria-label="Loading"
      className="grid grid-cols-[repeat(auto-fill,minmax(260px,1fr))] gap-3"
    >
      {Array.from({ length: cards }).map((_, i) => (
        <CardSkeleton key={i} />
      ))}
    </div>
  );
}

/** A small inline block skeleton (e.g. for panels / cards). */
export function BlockSkeleton({ height = 120 }: { height?: number }) {
  return (
    <ContentLoader
      speed={1.5}
      width="100%"
      height={height}
      viewBox={`0 0 400 ${height}`}
      preserveAspectRatio="none"
      backgroundColor={BG}
      foregroundColor={FG}
    >
      <rect x="0" y="0" rx="8" ry="8" width="400" height={height} />
    </ContentLoader>
  );
}
