scoreboard players set #wv_t arnis.ds 0
scoreboard players add #wv_lv arnis.ds 1
execute if score #wv_lv arnis.ds >= #wv_max arnis.ds run scoreboard players set #wv_dir arnis.ds 0
execute if score #wv_dir arnis.ds matches 0 run data modify storage arnis:ds status set value "at its peak"
