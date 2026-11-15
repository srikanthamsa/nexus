import { Link as Link2, Users } from "@phosphor-icons/react";
import { Button } from "../components/Button";
import { PageHeader } from "../components/PageHeader";

export function FriendsScreen({ onDirectConnect }: { onDirectConnect: () => void }) {
  return (
    <div className="screen">
      <PageHeader
        title="Friends"
        description="Accounts and friend presence arrive in a later phase."
      />

      <div className="empty-state">
        <span className="empty-state__icon">
          <Users size={22} />
        </span>
        <h3>Friends arrive with accounts</h3>
        <p>
          There is no account service yet, so there are no friends, requests, or presence to show.
          Until that phase lands, share an invite code with the person you want to connect to.
        </p>
        <div className="empty-state__actions">
          <Button variant="primary" className="btn--light" leftIcon={<Link2 size={15} />} onClick={onDirectConnect}>
            Direct connect
          </Button>
        </div>
      </div>
    </div>
  );
}
