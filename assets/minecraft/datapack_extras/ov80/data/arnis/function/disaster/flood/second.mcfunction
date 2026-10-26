function arnis:disaster/water/step
execute unless score #lv_flood arnis.ds = #wv_lv arnis.ds run function arnis:disaster/flood/level
scoreboard players add #cd arnis.ds 1
execute if score #cd arnis.ds matches 5.. if score #wv_dir arnis.ds matches 0..1 run function arnis:disaster/flood/drowned
