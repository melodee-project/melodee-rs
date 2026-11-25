import { UserAvatar } from '@carbon/icons-react';
import { useEffect, useState } from 'react';

import { USER_AVATAR_CHANGED_EVENT } from '../api/client';

/** The browser session resolves the image even when there is no stored username. */
export function SessionAvatar({ username }: { username: string | null }) {
  const [version, setVersion] = useState(Date.now);
  const [failedSrc, setFailedSrc] = useState<string>();
  const src = `/api/admin/v1/session/avatar?v=${version}`;

  useEffect(() => {
    const refresh = () => setVersion((previous) => Math.max(Date.now(), previous + 1));
    window.addEventListener(USER_AVATAR_CHANGED_EVENT, refresh);
    return () => window.removeEventListener(USER_AVATAR_CHANGED_EVENT, refresh);
  }, []);

  if (failedSrc === src && !username) return <UserAvatar size={20} aria-hidden />;
  return (
    <span className="melodee-shell__avatar" aria-hidden>
      {failedSrc === src
        ? username?.slice(0, 1).toUpperCase()
        : <img key={src} src={src} alt="" onError={() => setFailedSrc(src)} />}
    </span>
  );
}
