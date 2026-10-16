function arnis:outbreak/clear
# A new run id, never reused, so the infected of every earlier run are cleared wherever they
# turn up.
scoreboard players add #run arnis.ob 1
scoreboard players set #state arnis.ob 1
scoreboard players set #phase arnis.ob 1
scoreboard players set #day arnis.ob 1
scoreboard players set #night arnis.ob 0
scoreboard players set #won arnis.ob 0
scoreboard players set #spawn_left arnis.ob 0
scoreboard players set #wave_total arnis.ob 1
scoreboard players reset * arnis.ob.seen
scoreboard players reset * arnis.ob.kills
scoreboard players reset * arnis.ob.deaths
scoreboard objectives setdisplay sidebar arnis.ob.kills
bossbar set arnis:outbreak visible true
time set 0
difficulty normal
gamemode survival @a[gamemode=creative]
