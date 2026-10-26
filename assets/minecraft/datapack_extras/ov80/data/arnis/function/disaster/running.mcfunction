# The disasters play out in the mapped area, so players in the Nether or the End are out of them.
execute in minecraft:overworld store result score #players arnis.ds if entity @a[gamemode=!spectator,distance=0..]
execute as @a unless score @s arnis.ds.seen matches 1.. at @s run function arnis:disaster/intro
scoreboard players operation #was arnis.ds = #phase arnis.ds
execute if score #was arnis.ds matches 1 run function arnis:disaster/calm
execute if score #was arnis.ds matches 2 run function arnis:disaster/warning
execute if score #was arnis.ds matches 3 run function arnis:disaster/active
function arnis:disaster/hud/update
