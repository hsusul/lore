import { Component, type ErrorInfo, type ReactNode } from "react";

interface Props {
  children: ReactNode;
  onReset?: () => void;
}

interface State {
  error: Error | null;
}

/** Contain a render crash to the region that threw, with a way back. */
export default class ErrorBoundary extends Component<Props, State> {
  public override state: State = { error: null };

  public static getDerivedStateFromError(error: Error): State {
    return { error };
  }

  public override componentDidCatch(error: Error, info: ErrorInfo) {
    // Local only: Lore sends no telemetry.
    console.error("Render error:", error, info);
  }

  private reset = () => {
    this.setState({ error: null });
    this.props.onReset?.();
  };

  public override render() {
    if (!this.state.error) return this.props.children;
    return (
      <div className="crash" role="alert">
        <p className="subhead">Something went wrong</p>
        <p className="quiet">{this.state.error.message || "This part of Lore failed to draw."}</p>
        <div className="crash__actions">
          <button type="button" className="btn btn--primary" onClick={this.reset}>
            Try again
          </button>
          <button type="button" className="btn" onClick={() => window.location.reload()}>
            Reload window
          </button>
        </div>
      </div>
    );
  }
}
