import { Tooltip } from "@heroui/react";

/**
 * A container-image reference, rendered safely: always one line, ellipsized
 * by CSS rather than clipped by character count, with the full reference in a
 * tooltip on hover. Long nixery refs (a dozen packages joined by slashes) are
 * the norm here, not the exception — every place a ref lands must assume it
 * can be 100+ characters.
 */
export default function ImageRef({
  value,
  className = "",
}: {
  value?: string | null;
  className?: string;
}) {
  if (!value) return null;
  return (
    <Tooltip
      content={
        <span className="max-w-[440px] break-all font-mono text-xs">{value}</span>
      }
      delay={300}
      closeDelay={0}
    >
      <span className={`block min-w-0 max-w-full truncate ${className}`}>
        {value}
      </span>
    </Tooltip>
  );
}
