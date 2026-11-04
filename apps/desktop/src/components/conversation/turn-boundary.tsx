import { Component, type ErrorInfo, type ReactNode } from "react";

import { ErrorState } from "@/components/conversation/elements/error-state";

/**
 * One message of the thread that fails to render shows "This turn couldn't render" with Try
 * again, in its place: the rest of the thread stays. Try again renders it afresh. Around a whole
 * view (`summary`), its error never blanks the window: the sidebar and the rest stay.
 */
export class TurnBoundary extends Component<{ children: ReactNode; summary?: string }, { error: Error | null }> {
  override state: { error: Error | null } = { error: null };

  static getDerivedStateFromError(error: Error): { error: Error } {
    return { error };
  }

  override componentDidCatch(error: Error, info: ErrorInfo): void {
    console.error("A turn failed to render", error, info.componentStack);
  }

  override render(): ReactNode {
    const { error } = this.state;
    if (!error) return this.props.children;
    return (
      <div data-slot="turn-render-error">
        <ErrorState summary={this.props.summary ?? "This turn couldn’t render."} details={error.message} onRetry={() => this.setState({ error: null })} />
      </div>
    );
  }
}
