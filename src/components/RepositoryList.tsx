import type { RepositorySummary } from "../ipc";

export default function RepositoryList({
  repositories,
  selectedId,
  onSelect,
}: {
  repositories: RepositorySummary[];
  selectedId: string | null;
  onSelect: (id: string | null) => void;
}) {
  return (
    <nav className="repos" aria-label="repositories">
      <button
        className="nav-item"
        aria-pressed={selectedId === null}
        onClick={() => onSelect(null)}
      >
        <span className="dot dot--accent" aria-hidden />
        <span className="nav-item__name">All sessions</span>
      </button>

      <h2 className="nav-heading">Repositories</h2>
      {repositories.length === 0 ? (
        <p className="repos__empty">None resolved yet.</p>
      ) : (
        <ul className="nav-list">
          {repositories.map((repo) => (
            <li key={repo.id}>
              <button
                className="nav-item"
                aria-pressed={selectedId === repo.id}
                onClick={() => onSelect(repo.id)}
              >
                <span
                  className={`dot dot--${repo.identity_confidence}`}
                  aria-label={`${repo.identity_confidence} confidence`}
                  title={`${repo.identity_confidence} confidence`}
                />
                <span className="nav-item__name">{repo.display_name}</span>
                {repo.is_missing && <span className="chip chip--danger">missing</span>}
                <span className="nav-item__count">{repo.session_count}</span>
              </button>
            </li>
          ))}
        </ul>
      )}
    </nav>
  );
}
