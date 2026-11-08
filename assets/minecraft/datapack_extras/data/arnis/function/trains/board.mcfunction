data remove entity @s interaction
# On a boat the seat is up on deck, over the box that was clicked.
execute if entity @s[tag=arnis.tdeck] at @s positioned ~-1.5 ~-1 ~-1.5 if entity @e[type=block_display,tag=arnis.seat,tag=arnis.tdeck,dx=2,dy=96,dz=2] run return run ride @p[tag=arnis.boarding] mount @e[type=block_display,tag=arnis.seat,tag=arnis.tdeck,dx=2,dy=96,dz=2,sort=nearest,limit=1]
execute at @s run ride @p[tag=arnis.boarding] mount @e[type=block_display,tag=arnis.seat,distance=..8,sort=nearest,limit=1]
