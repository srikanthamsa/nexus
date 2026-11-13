import { useEffect, useState, type FormEvent } from "react";
import { ArrowRight, Cpu, Radio, ShieldCheck, Zap } from "lucide-react";
import { Button } from "../components/Button";
import { Logo, LogoMark } from "../components/Logo";
import { TextField } from "../components/TextField";
import { getSystemInfo } from "../lib/native";
import { useApp } from "../lib/store";
import { friendlyDeviceName } from "../lib/utils";

export function SignInScreen() {
  const { signIn } = useApp();
  const [name, setName] = useState("");
  const [nameError, setNameError] = useState<string>();
  const NAME_LIMIT = 20;

  useEffect(() => {
    let cancelled = false;
    void getSystemInfo().then((info) => {
      if (!cancelled && info.hostname) setName(friendlyDeviceName(info.hostname));
    });
    return () => {
      cancelled = true;
    };
  }, []);

  const submit = (event?: FormEvent) => {
    event?.preventDefault();
    if (!name.trim()) {
      setNameError("Give this Mac a display name.");
      return;
    }
    if (name.trim().length > NAME_LIMIT) {
      setNameError(`Keep the display name to ${NAME_LIMIT} characters or fewer.`);
      return;
    }
    setNameError(undefined);
    signIn(name);
  };

  return (
    <div className="auth">
      <section className="auth__visual">
        <div className="auth__mesh" aria-hidden="true" />
        <div className="auth__grid" aria-hidden="true" />

        <div className="auth__visual-inner">
          <div className="auth__eyebrow">
            <span className="auth__eyebrow-rule" aria-hidden="true" />
            ParsecPlus for macOS
          </div>
          <h1 className="auth__headline">
            Every screen.
            <br />
            <span>Zero friction.</span>
          </h1>
          <p className="auth__subhead">
            Your fastest machines, wherever you are. Direct paths, crystal-clear color, and input
            that feels local.
          </p>

          <div className="auth-map" aria-hidden="true">
            <svg
              className="auth-map__lines"
              viewBox="0 0 100 100"
              preserveAspectRatio="none"
              fill="none"
            >
              <path className="auth-map__line" d="M50 47 C38 32 24 22 13 16" />
              <path className="auth-map__line auth-map__line--active" d="M50 47 C64 31 74 20 84 14" />
              <path className="auth-map__line" d="M50 47 C66 62 74 72 82 80" />
              <circle className="auth-map__pulse" cx="50" cy="47" r="7" />
            </svg>

            <div className="auth-node auth-node--one">
              <span className="device-dot is-live" />
              <span className="auth-node__name">Home Mac</span>
              <span className="auth-node__meta">Personal</span>
            </div>

            <div className="auth-node auth-node--center">
              <LogoMark size={28} />
              <span className="auth-node__name">{name.trim() || "This Mac"}</span>
              <span className="auth-node__meta">Local profile · ready</span>
            </div>

            <div className="auth-node auth-node--two">
              <span className="device-dot is-live" />
              <span className="auth-node__name">Office Workstation</span>
              <span className="auth-node__meta">Work</span>
            </div>

            <div className="auth-node auth-node--three">
              <span className="device-dot is-away" />
              <span className="auth-node__name">Studio Display</span>
              <span className="auth-node__meta">5K pro display</span>
            </div>

            <div className="auth-route-pill">
              <span className="auth-route-pill__dot" />
              End-to-end encrypted
              <span className="auth-route-pill__sep" />
              No account required
            </div>
          </div>

          <ul className="auth__features">
            <li>
              <span>
                <Radio size={11} />
              </span>
              Direct-first networking
            </li>
            <li>
              <span>
                <ShieldCheck size={11} />
              </span>
              End-to-end encryption
            </li>
            <li>
              <span>
                <Zap size={11} />
              </span>
              Ultra low latency
            </li>
            <li>
              <span>
                <Cpu size={11} />
              </span>
              Hardware HEVC
            </li>
          </ul>
        </div>
      </section>

      <section className="auth__panel">
        <div className="auth__panel-inner">
          <Logo size={26} className="auth__brand" />
          <div className="auth__form-head">
            <span className="auth__form-eyebrow">Local profile</span>
            <h2>Welcome to ParsecPlus</h2>
            <p>
              Set up this Mac to connect to your computers. No email, password or account required
              — this profile stays on this device.
            </p>
          </div>

          <form className="auth__form" onSubmit={submit} noValidate>
            <TextField
              label="Display name"
              placeholder="This Mac"
              autoComplete="name"
              value={name}
              error={nameError}
              hint={`Up to ${NAME_LIMIT} characters.`}
              containerClassName={nameError ? "field--wiggle" : undefined}
              onChange={(event) => {
                const next = event.target.value;
                setName(next);
                setNameError(
                  next.trim().length > NAME_LIMIT
                    ? `Keep the display name to ${NAME_LIMIT} characters or fewer.`
                    : undefined,
                );
              }}
            />

            <Button
              type="submit"
              variant="primary"
              size="lg"
              block
              rightIcon={<ArrowRight size={16} />}
            >
              Continue
            </Button>
          </form>

          <div className="auth__trust">
            <ShieldCheck size={14} />
            Local profile only · no account · no telemetry
          </div>

          <p className="auth__legal">
            An independent clean-room project. Not affiliated with Parsec Technologies.
          </p>
        </div>
      </section>
    </div>
  );
}
