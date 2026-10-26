$data modify storage arnis:ds scan.map set value "$(map)"
scoreboard players set #budget arnis.ds 1
execute in minecraft:overworld as @a[distance=0..] at @s run function arnis:disaster/water/around
$execute if score #budget arnis.ds matches 1 run scoreboard players set #wd_$(map) arnis.ds 0
# Once the water near everyone has been gone for half a minute, and what spilled as it
# drained has dried up, sources can form again as they did before.
execute if score #budget arnis.ds matches 0 run scoreboard players set #wsc_idle arnis.ds 0
execute if score #budget arnis.ds matches 1 if score #wsc arnis.ds matches 0.. if score #wv_on arnis.ds matches 0 if score #lv_flood arnis.ds matches 0 if score #lv_tsunami arnis.ds matches 0 run scoreboard players add #wsc_idle arnis.ds 1
execute if score #wsc_idle arnis.ds matches 20.. run function arnis:disaster/water/release
