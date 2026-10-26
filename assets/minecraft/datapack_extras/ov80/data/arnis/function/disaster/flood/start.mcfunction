scoreboard players set #wv_max arnis.ds 5
execute if score #pass arnis.ds matches 2.. run scoreboard players set #wv_max arnis.ds 8
scoreboard players operation #wv_max arnis.ds < #k_flood arnis.ds
scoreboard players set #wv_step arnis.ds 6
scoreboard players set #wv_hold arnis.ds 25
scoreboard players set #wv_fall arnis.ds 3
function arnis:disaster/water/begin
function arnis:disaster/flood/level
