import { UsersRound } from "lucide-react";
import { PageHeader } from "../components/PageHeader";

export function TeamsScreen() {
  return (
    <div className="screen">
      <PageHeader
        title="Teams"
        description="Shared machines, roles, and invites arrive with accounts."
      />

      <div className="empty-state">
        <span className="empty-state__icon">
          <UsersRound size={22} />
        </span>
        <h3>Teams arrive with accounts</h3>
        <p>
          Team membership, roles, and shared computers need the account service planned for a
          later phase. There are no teams on this Mac yet, and nothing is seeded or simulated.
        </p>
      </div>
    </div>
  );
}
