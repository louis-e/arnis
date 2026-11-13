# Once a second: walkers out of everyone's reach are gone, and new ones step out onto the
# ways around each player, fewer by night.
scoreboard players set #walk_r arnis 0
execute as @e[tag=arnis.walker] at @s unless entity @a[distance=..112] run function arnis:walk/gone
function arnis:util/daytime
scoreboard players operation #cap arnis.walk = #day_cap arnis.walk
execute if score #daytime arnis matches 13000..22999 run scoreboard players operation #cap arnis.walk = #night_cap arnis.walk
execute if score #lanes arnis.walk matches 1.. as @a at @s run function arnis:walk/near
