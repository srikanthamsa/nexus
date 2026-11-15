import { Clock as Clock3, Cloud, GameController as Gamepad2, Trophy, UsersThree as UsersRound } from "@phosphor-icons/react";
import { PageHeader } from "../components/PageHeader";

const tiles = [
  {
    icon: <Cloud size={22} />,
    title: "Cloud play",
    description:
      "Jump into a game already running on your gaming PC, from any screen you own.",
  },
  {
    icon: <UsersRound size={22} />,
    title: "Couch co-op",
    description:
      "Pass a controller to a friend in another city on the same low-latency session.",
  },
  {
    icon: <Trophy size={22} />,
    title: "Tournaments",
    description:
      "Brackets and lobbies for the Night Shift crew, once the arcade opens.",
  },
];

export function ArcadeScreen() {
  return (
    <div className="screen arcade-screen">
      <PageHeader
        title="Arcade"
        description="Play together, right from a Nexus session."
        meta={<span className="chip">Coming soon</span>}
      />

      <section className="arcade-hero" aria-labelledby="arcade-title">
        <div className="arcade-hero__glow" aria-hidden="true" />
        <div className="arcade-cabinet" aria-hidden="true">
          <div className="arcade-cabinet__marquee">ARCADE</div>
          <div className="arcade-cabinet__screen">
            <Gamepad2 size={44} weight="regular" />
          </div>
          <div className="arcade-cabinet__controls">
            <span className="arcade-cabinet__stick" />
            <span className="arcade-cabinet__button" />
            <span className="arcade-cabinet__button is-alt" />
            <span className="arcade-cabinet__button is-cool" />
          </div>
          <div className="arcade-cabinet__base" />
        </div>

        <div className="arcade-hero__copy">
          <span className="arcade-kicker">
            <Clock3 size={13} />
            Coming soon
          </span>
          <h2 id="arcade-title">ARCADE</h2>
          <p>
            Cloud play, couch co-op, and tournaments built on the same low-latency sessions you
            already use. Nothing here is wired up yet — it lands in a later phase.
          </p>
          <div className="arcade-hero__chips">
            <span className="mini-chip">
              <Gamepad2 size={11} />
              Controller-ready
            </span>
            <span className="mini-chip">
              <Cloud size={11} />
              Hosted sessions
            </span>
          </div>
        </div>
      </section>

      <div className="arcade-grid">
        {tiles.map((tile) => (
          <article className="arcade-tile" key={tile.title} aria-disabled="true">
            <span className="arcade-tile__icon">{tile.icon}</span>
            <h3>{tile.title}</h3>
            <p>{tile.description}</p>
            <span className="arcade-tile__badge">Coming soon</span>
          </article>
        ))}
      </div>
    </div>
  );
}
