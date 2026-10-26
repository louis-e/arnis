scoreboard players operation #lv_flood arnis.ds = #wv_lv arnis.ds
execute if score #lv_flood arnis.ds matches ..-1 run scoreboard players set #lv_flood arnis.ds 0
function arnis:disaster/flood/tops
execute in minecraft:overworld as @a[distance=0..] at @s run playsound minecraft:ambient.underwater.enter ambient @s ~ ~ ~ 0.7 0.6
