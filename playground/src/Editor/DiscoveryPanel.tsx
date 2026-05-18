import type { DiscoveredFile } from "./types";

interface Props {
  discovery: DiscoveredFile | null;
}

export function DiscoveryPanel({ discovery }: Props) {
  if (!discovery) {
    return (
      <div className="p-4 text-text-dim text-sm">
        Write some tests to see discovery results.
      </div>
    );
  }

  const tests = discovery.parsed.tests ?? [];
  const hooks = discovery.parsed.hooks ?? [];
  const errors = discovery.parsed.errors ?? [];

  return (
    <div className="p-3 text-sm overflow-auto h-full">
      {errors.length > 0 && (
        <div className="mb-3">
          <h3 className="text-red font-bold mb-1">Errors</h3>
          {errors.map((err, i) => (
            <div key={i} className="text-red/80 ml-2">
              {err}
            </div>
          ))}
        </div>
      )}

      <div className="mb-3">
        <h3 className="text-text font-bold mb-1">
          Tests ({tests.length})
        </h3>
        {tests.length === 0 ? (
          <div className="text-text-dim ml-2">No tests found.</div>
        ) : (
          <ul className="ml-2 space-y-0.5">
            {tests.map((t, i) => (
              <li key={i} className="flex items-center gap-2">
                <span className="text-green">&#x25cf;</span>
                <span className="text-text">
                  {t.display_name ?? t.name}
                  {t.case_label ? `[${t.case_label}]` : ""}
                </span>
                {t.line_number != null && (
                  <span className="text-text-dim">:{t.line_number}</span>
                )}
                {t.skip != null && (
                  <span className="text-yellow text-xs px-1 rounded bg-yellow/10">
                    skip
                  </span>
                )}
                {t.todo != null && (
                  <span className="text-accent text-xs px-1 rounded bg-accent/10">
                    todo
                  </span>
                )}
                {t.xfail != null && (
                  <span className="text-text-dim text-xs px-1 rounded bg-text-dim/10">
                    xfail
                  </span>
                )}
                {(t.expected_assertions?.length ?? 0) > 0 && (
                  <span className="text-text-dim text-xs">
                    ({t.expected_assertions?.length ?? 0} assertions)
                  </span>
                )}
              </li>
            ))}
          </ul>
        )}
      </div>

      {hooks.length > 0 && (
        <div className="mb-3">
          <h3 className="text-text font-bold mb-1">
            Hooks ({hooks.length})
          </h3>
          <ul className="ml-2 space-y-0.5">
            {hooks.map((h, i) => (
              <li key={i} className="flex items-center gap-2">
                <span className="text-accent">&#x25cf;</span>
                <span className="text-text">{h.name}</span>
                <span className="text-text-dim text-xs">
                  per:{h.per}
                </span>
                {(h.depends_on?.length ?? 0) > 0 && (
                  <span className="text-text-dim text-xs">
                    deps: {h.depends_on?.join(", ")}
                  </span>
                )}
              </li>
            ))}
          </ul>
        </div>
      )}

      {discovery.dynamic_imports && (
        <div className="text-yellow text-xs mt-2">
          Dynamic imports detected — this file will always re-run with
          --changed.
        </div>
      )}
    </div>
  );
}
