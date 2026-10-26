function arnis:disaster/clear
# A new id, never reused, so what every earlier run spawned is cleared wherever it turns up.
scoreboard players add #run arnis.ds 1
scoreboard players set #state arnis.ds 1
scoreboard players set #phase arnis.ds 1
scoreboard players set #t arnis.ds 60
scoreboard players set #calm_len arnis.ds 60
scoreboard players set #kind arnis.ds 0
scoreboard players set #pass arnis.ds 0
scoreboard players set #won arnis.ds 0
data modify storage arnis:ds bag set value []
scoreboard players reset * arnis.ds.seen
scoreboard players reset * arnis.ds.won
scoreboard players reset * arnis.ds.deaths
scoreboard objectives setdisplay sidebar arnis.ds.won
bossbar set arnis:disaster visible true
difficulty normal
gamemode survival @a[gamemode=creative]
