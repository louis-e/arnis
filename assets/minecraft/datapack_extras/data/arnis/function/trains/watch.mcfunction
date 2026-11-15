# A spectator who clicked a carriage's, a boat's or an aircraft's box goes over to its
# model, which glides where the box jumps from step to step. Every box is nearer its own
# model than any other.
tag @s add arnis.watching
execute as @e[type=item_display,tag=arnis.tp,distance=..72,sort=nearest,limit=1] run spectate @s @a[tag=arnis.watching,limit=1]
tag @s remove arnis.watching
