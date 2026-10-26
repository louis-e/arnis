scoreboard players operation #lv_tsunami arnis.ds = #wv_lv arnis.ds
execute if score #lv_tsunami arnis.ds matches ..-1 run scoreboard players set #lv_tsunami arnis.ds 0
function arnis:disaster/tsunami/tops
execute if score #wv_dir arnis.ds matches 1 in minecraft:overworld as @a[distance=0..] at @s run playsound minecraft:entity.generic.splash ambient @s ~ ~ ~ 1 0.5
