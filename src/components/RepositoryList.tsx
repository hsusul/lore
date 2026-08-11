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
        className="repos__all"
        aria-pressed={selectedId === null}
        onClick={() => onSelect(null)}
      >
        All sessions
      </button>
      <h2 className="repos__heading">Repositories</h2>
      {repositories.length === 0 ? (
        <p className="repos__empty">None resolved yet.</p>
      ) : (
        <ul>
          {repositories.map((repo) => (
            <li key={repo.id}>
              <button
                aria-pressed={selectedId === repo.id}
                onClick={() => onSelect(repo.id)}
                className="repos__item"
              >
                <span className="repos__name">{repo.display_name}</span>
                <span className={`conf conf--${repo.identity_confidence}`}>
                  {repo.identity_confidence}
                </span>
                {repo.is_missing && <span className="badge badge--missing">missing</span>}
                <span className="repos__count">{repo.session_count}</span>
              </button>
            </li>
          ))}
        </ul>
      )}
    </nav>
  );
}
