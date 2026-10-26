# Moves #wv_lv one level at a time: up to #wv_max, held there, then back down to dry.
scoreboard players add #wv_t arnis.ds 1
execute if score #wv_dir arnis.ds matches 1 if score #wv_t arnis.ds >= #wv_step arnis.ds run function arnis:disaster/water/up
execute if score #wv_dir arnis.ds matches 0 if score #wv_t arnis.ds >= #wv_hold arnis.ds run function arnis:disaster/water/turn
execute if score #wv_dir arnis.ds matches -1 if score #wv_t arnis.ds >= #wv_fall arnis.ds run function arnis:disaster/water/down
scoreboard players operation #bar arnis.ds = #wv_lv arnis.ds
scoreboard players operation #bar_max arnis.ds = #wv_max arnis.ds
