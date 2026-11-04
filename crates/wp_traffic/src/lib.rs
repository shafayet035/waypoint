//! AI traffic. NPC vehicles drive the same vehicle model as the player:
//! they plan routes on the lane graph, follow their lane, keep their
//! distance to the vehicle ahead (IDM) and take turns at junctions.

pub mod control;
