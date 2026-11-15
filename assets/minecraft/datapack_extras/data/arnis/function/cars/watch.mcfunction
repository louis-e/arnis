# A spectator who clicked a car's box goes over to its model, which glides where the box
# jumps from step to step.
tag @s add arnis.watching
execute as @e[type=item_display,tag=arnis.car.lift,distance=..1.5,sort=nearest,limit=1] run spectate @s @a[tag=arnis.watching,limit=1]
tag @s remove arnis.watching
