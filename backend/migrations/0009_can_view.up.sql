-- 0009 can_view (04 section 5.10)
CREATE FUNCTION app.can_view(p_user_id uuid, p_station_id uuid) RETURNS boolean
LANGUAGE sql STABLE SECURITY INVOKER PARALLEL SAFE SET search_path = pg_catalog AS
$$
  SELECT EXISTS (
    SELECT 1
    FROM app.users v
    JOIN app.stations s ON s.id = p_station_id
    WHERE v.id = p_user_id
      AND v.is_active
      AND (
        s.owner_id = v.id
        OR EXISTS (
          SELECT 1 FROM app.follows f
          WHERE f.follower_id = v.id AND f.followee_id = s.owner_id AND f.status = 'approved'
        )
      )
  )
$$;

CREATE FUNCTION app.visible_station_ids(p_user_id uuid) RETURNS SETOF uuid
LANGUAGE sql STABLE SECURITY INVOKER PARALLEL SAFE SET search_path = pg_catalog AS
$$
  SELECT s.id
  FROM app.users v
  JOIN app.stations s ON (
    s.owner_id = v.id
    OR EXISTS (SELECT 1 FROM app.follows f
               WHERE f.follower_id = v.id AND f.followee_id = s.owner_id AND f.status = 'approved'))
  WHERE v.id = p_user_id AND v.is_active
$$;

REVOKE EXECUTE ON FUNCTION app.can_view(uuid, uuid), app.visible_station_ids(uuid) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION app.can_view(uuid, uuid), app.visible_station_ids(uuid) TO app_api;
